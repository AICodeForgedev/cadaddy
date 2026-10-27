//! A plot's stroked block attributes are searchable only on a layout's own paper (its title
//! block's sheet number). Model space's (a device symbol's mark), plotted directly or seen
//! through a viewport, stays strokes only: read back from the PDF, an extra word beside a symbol
//! changes what the print import finds.

use cadaddy_doc::{
    Attrib, AttribModes, AttribProps, Block, Circle, Common, Drawing, Entity, EntityKind, HAlign, Insert, Space, Text, VAlign, Viewport,
};
use cadaddy_geom::{Vec2, Vec3};
use cadaddy_render::{Options, build_plot};

fn tagged(block: &str, at: [f64; 2], tag: &str, value: &str) -> EntityKind {
    let text = Text {
        insert: Vec3::new(at[0] + 0.3, at[1], 0.0),
        align_pt: None,
        height: 0.1,
        value: value.into(),
        rotation: 0.0,
        width_factor: 1.0,
        oblique: 0.0,
        style: "Standard".into(),
        halign: HAlign::default(),
        valign: VAlign::default(),
    };
    let attrib = Attrib {
        tag: tag.into(),
        text,
        invisible: false,
        constant: false,
        prompt: String::new(),
        props: AttribProps::default(),
        multiline: None,
        modes: AttribModes::default(),
    };
    EntityKind::Insert(Insert {
        block: block.into(),
        insert: Vec3::new(at[0], at[1], 0.0),
        scale: Vec3::new(1.0, 1.0, 1.0),
        rotation: 0.0,
        attribs: vec![attrib],
        cols: 1,
        rows: 1,
        col_spacing: 0.0,
        row_spacing: 0.0,
        clip: None,
    })
}

/// A device symbol tagged `E7` in model space, and Layout1 with a viewport onto it and a title
/// block numbered `A-101`.
fn drawing() -> Drawing {
    let mut d = Drawing::default();
    let mut b = Block::new("SYM");
    b.entities.push(Entity {
        handle: d.new_handle(),
        common: Common::default(),
        kind: EntityKind::Circle(Circle { center: Vec3::ZERO, radius: 0.1 }),
    });
    d.blocks.insert("SYM".into(), std::sync::Arc::new(b));
    d.add(&Space::Model, Common::default(), tagged("SYM", [2.0, 2.0], "TAG", "E7")).unwrap();
    let paper = Space::Paper("Layout1".into());
    d.add(&paper, Common::default(), tagged("SYM", [30.0, 2.0], "SHEET_NO", "A-101")).unwrap();
    let vp = Viewport {
        center: Vec3::new(10.0, 10.0, 0.0),
        width: 16.0,
        height: 16.0,
        view_center: Vec2::new(2.0, 2.0),
        view_height: 8.0,
        id: 2,
        locked: false,
        frozen_layers: Vec::new(),
        layer_colors: Vec::new(),
        layer_linetypes: Vec::new(),
        layer_lineweights: Vec::new(),
        clip: None,
        on: true,
        twist: 0.0,
        anno_scale: None,
    };
    d.add(&paper, Common::default(), EntityKind::Viewport(Box::new(vp))).unwrap();
    d
}

fn words(d: &Drawing, space: &Space) -> Vec<String> {
    build_plot(d, space, &Options::default()).words.into_iter().map(|w| w.text).collect()
}

#[test]
fn only_a_layouts_own_stroked_attributes_are_searchable() {
    let d = drawing();
    let model = words(&d, &Space::Model);
    assert!(!model.iter().any(|w| w.contains("E7")), "a model plot's device mark became a word: {model:?}");
    let sheet = words(&d, &Space::Paper("Layout1".into()));
    assert!(sheet.iter().any(|w| w.contains("A-101")), "the title block's sheet number is not searchable: {sheet:?}");
    assert!(!sheet.iter().any(|w| w.contains("E7")), "a device mark seen through the viewport became a word: {sheet:?}");
}
