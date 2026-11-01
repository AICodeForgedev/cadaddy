//! VP Lineweight: the same model line seen through two viewports is drawn with the layer's
//! lineweight in one and with the viewport's override in the other, and only there.

use cadaddy_doc::{Common, Drawing, EntityKind, Line, Lineweight, Space, Viewport};
use cadaddy_geom::{Vec2, Vec3};
use cadaddy_render::{Kind, Options, VIEWPORT_CONTENT, build};

/// A viewport 8 x 6 paper units wide centred at paper x = `x`, looking at the model line.
fn viewport(id: u32, x: f64, lineweights: Vec<(String, Lineweight)>) -> EntityKind {
    EntityKind::Viewport(Box::new(Viewport {
        center: Vec3::new(x, 4.0, 0.0),
        width: 8.0,
        height: 6.0,
        view_center: Vec2::new(5.0, 0.0),
        view_height: 6.0,
        id,
        locked: false,
        frozen_layers: Vec::new(),
        layer_colors: Vec::new(),
        layer_linetypes: Vec::new(),
        layer_lineweights: lineweights,
        clip: None,
        on: true,
        twist: 0.0,
        anno_scale: None,
    }))
}

#[test]
fn a_viewport_lineweight_override_changes_the_width_in_that_viewport_only() {
    let mut d = Drawing::new_metric();
    d.ensure_layer("Walls");
    d.layer_mut("Walls").unwrap().lineweight = Lineweight::Mm100(25);
    let line = EntityKind::Line(Line { a: Vec3::new(0.0, 0.0, 0.0), b: Vec3::new(10.0, 0.0, 0.0) });
    d.add(&Space::Model, Common { layer: "Walls".into(), ..Default::default() }, line).unwrap();
    let paper = Space::Paper("Layout1".into());
    // Viewport 2 (left, x = 5) has no override; viewport 3 (right, x = 25) draws Walls at 0.70 mm.
    d.add(&paper, Common::default(), viewport(2, 5.0, Vec::new())).unwrap();
    d.add(&paper, Common::default(), viewport(3, 25.0, vec![("walls".into(), Lineweight::Mm100(70))])).unwrap();

    let list = build(&d, &paper, &Options { lineweights: true, ..Default::default() });
    // (mean paper x, width) of every model line piece drawn through a viewport.
    let mut drawn: Vec<(bool, f32)> = list
        .prims
        .iter()
        .filter(|p| p.handle == VIEWPORT_CONTENT && p.kind == Kind::Polyline)
        .map(|p| {
            let pts = &list.verts[p.start as usize..(p.start + p.len) as usize];
            let mean = pts.iter().map(|v| v.x).sum::<f64>() / pts.len() as f64;
            (mean > 15.0, p.lw)
        })
        .collect();
    drawn.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert_eq!(drawn, vec![(false, 0.25), (true, 0.7)], "left viewport: the layer's 0.25 mm; right viewport: the override 0.70 mm");
}
