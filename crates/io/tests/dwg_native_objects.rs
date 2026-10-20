//! A DWG whose table records and objects are read with CADaddy's own codec makes the same
//! drawing as through the DWG bridge: layers with their states, layouts and their viewports,
//! groups, UCSs, named views and styles (Q200).

use cadaddy_doc::*;
use cadaddy_geom::{Vec2, Vec3};
use serde_json::Value as J;

fn drawing() -> Drawing {
    let mut d = Drawing::new_metric();
    let mut walls = Layer::new("WALLS");
    walls.color = cadaddy_color::Color::Index(1);
    walls.lineweight = Lineweight::Mm100(50);
    let mut hidden = Layer::new("HIDDEN");
    hidden.on = false;
    hidden.plot = false;
    let mut frozen = Layer::new("FROZEN");
    frozen.frozen = true;
    frozen.locked = true;
    frozen.color = cadaddy_color::Color::True(cadaddy_color::Rgb(255, 128, 0));
    d.layers.extend([walls.clone(), hidden.clone(), frozen.clone()]);
    d.layer_states.push(LayerState::new("ALL ON", vec![walls.clone(), Layer { on: true, ..hidden.clone() }, frozen.clone()]));
    let mut st = LayerState::new("PLOT", vec![walls, hidden]);
    st.description = "for plotting".into();
    d.layer_states.push(st);
    d.ucss.push(Ucs { name: "TILTED".into(), origin: Vec3::new(5.0, 6.0, 0.0), x_axis: Vec3::new(0.6, 0.8, 0.0), y_axis: Vec3::new(-0.8, 0.6, 0.0) });
    let line = d
        .add(
            &Space::Model,
            Common { layer: "WALLS".into(), ..Common::default() },
            EntityKind::Line(Line { a: Vec3::ZERO, b: Vec3::new(10.0, 0.0, 0.0) }),
        )
        .unwrap();
    let circle = d.add(&Space::Model, Common::default(), EntityKind::Circle(Circle { center: Vec3::new(5.0, 5.0, 0.0), radius: 2.0 })).unwrap();
    d.groups.push(Group { name: "PAIR".into(), description: "a line and a circle".into(), selectable: true, members: vec![line, circle] });
    // A second layout with a viewport of its own.
    let mut l2 = Layout::new("Sheet 2", 2);
    l2.page.paper = "ISO A3".into();
    d.layouts.push(l2);
    let vp = Viewport {
        center: Vec3::new(150.0, 100.0, 0.0),
        width: 200.0,
        height: 120.0,
        view_center: Vec2::new(5.0, 5.0),
        view_height: 30.0,
        ..Viewport::default()
    };
    d.add(&Space::Paper("Sheet 2".into()), Common::default(), EntityKind::Viewport(vp)).unwrap();
    d
}

fn diff(a: &J, b: &J, path: &str) -> Option<String> {
    match (a, b) {
        (J::Number(x), J::Number(y)) => {
            let (x, y) = (x.as_f64().unwrap_or(0.0), y.as_f64().unwrap_or(0.0));
            ((x - y).abs() > 1e-9 * x.abs().max(1.0)).then(|| format!("{path}: {x} vs {y}"))
        }
        (J::Array(x), J::Array(y)) => {
            if x.len() != y.len() {
                return Some(format!("{path}: {} items vs {}", x.len(), y.len()));
            }
            x.iter().zip(y).enumerate().find_map(|(i, (p, q))| diff(p, q, &format!("{path}[{i}]")))
        }
        (J::Object(x), J::Object(y)) => {
            let keys: std::collections::BTreeSet<&String> = x.keys().chain(y.keys()).collect();
            keys.into_iter().find_map(|k| diff(x.get(k).unwrap_or(&J::Null), y.get(k).unwrap_or(&J::Null), &format!("{path}.{k}")))
        }
        _ => (a != b).then(|| format!("{path}: {a} vs {b}")),
    }
}

/// The drawing's tables and objects as JSON, part by part.
fn parts(d: &Drawing) -> J {
    serde_json::json!({
        "layers": d.layers,
        "linetypes": d.linetypes,
        "text_styles": d.text_styles,
        "dim_styles": d.dim_styles,
        "layer_states": d.layer_states,
        "layouts": d.layouts,
        "groups": d.groups,
        "views": d.views,
        "ucss": d.ucss,
        "mline_styles": d.mline_styles,
        "blocks": d.blocks.keys().collect::<Vec<_>>(),
    })
}

#[test]
fn tables_and_objects_read_natively_make_the_same_drawing() {
    let mut report = Vec::new();
    for v in ["AC1015", "AC1018", "AC1021", "AC1024", "AC1027", "AC1032"] {
        let mut d = drawing();
        d.header.set_str("ACADVER", v);
        let bytes = cadaddy_io::write(&d, "made-up.dwg").unwrap();
        let bridge = cadaddy_io::read(&bytes, "made-up.dwg").unwrap();
        let (native, rep) = cadaddy_io::dwg_native::read_with_native_entities(&bytes).unwrap();
        assert!(rep.failed.is_empty(), "{v}: {:?}", rep.failed);
        eprintln!("{v}: {} records from the codec", rep.replaced);
        // The whole drawing: tables, layer states, layouts (with their entities), groups, views.
        let mut a = parts(&bridge);
        let mut b = parts(&native);
        // DIMFXL isn't stored before R2007: the bridge makes it 0, the codec leaves DXF's default
        // (1). It only counts with DIMFXLON, which those versions don't have either.
        if matches!(v, "AC1015" | "AC1018") {
            for j in [&mut a, &mut b] {
                for s in j["dim_styles"].as_array_mut().into_iter().flatten() {
                    s.as_object_mut().map(|o| o.remove("fixedExtLength"));
                }
            }
        }
        if let Some(d) = diff(&a, &b, "") {
            report.push(format!("{v}: {d}"));
        }
        for (la, lb) in bridge.layouts.iter().zip(&native.layouts) {
            let ea: Vec<J> = la.entities.iter().map(|e| serde_json::to_value(e).unwrap()).collect();
            let eb: Vec<J> = lb.entities.iter().map(|e| serde_json::to_value(e).unwrap()).collect();
            if let Some(d) = diff(&J::Array(ea), &J::Array(eb), &format!("layout {}", la.name)) {
                report.push(format!("{v}: {d}"));
            }
        }
        // What was saved is what was read.
        assert_eq!(native.layer_states.len(), 2, "{v}: {report:?}");
        assert!(native.layouts.iter().any(|l| l.name == "Sheet 2" && !l.entities.is_empty()), "{v}");
        assert!(native.groups.iter().any(|g| g.name == "PAIR" && g.members.len() == 2), "{v}");
    }
    assert!(report.is_empty(), "{}", report.join("\n"));
}
