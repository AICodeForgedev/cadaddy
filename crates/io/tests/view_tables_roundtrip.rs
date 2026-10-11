//! DXF save → reopen keeps the VIEW table's paper space flag (with the view's layout), the view
//! twist and the layer snapshot (#463).

use cadcraft_doc::geom::Vec2;
use cadcraft_doc::{Drawing, NamedView};
use cadcraft_io::{read_dxf, write_dxf};

fn round_trip(d: &Drawing) -> Drawing {
    read_dxf(write_dxf(d).as_bytes()).expect("reopen")
}

#[test]
fn model_and_paper_views_round_trip() {
    let mut d = Drawing::new_imperial();
    let layout = d.layouts.first().map(|l| l.name.clone()).expect("a layout");
    d.views.push(NamedView { name: "Plan".into(), center: Vec2::new(5.0, 5.0), height: 20.0, width: 30.0, twist: 0.5, ..Default::default() });
    d.views.push(NamedView {
        name: "Sheet".into(),
        center: Vec2::new(4.0, 3.0),
        height: 8.0,
        width: 11.0,
        layer_state: Some("*VIEW Sheet".into()),
        layout: Some(layout),
        ..Default::default()
    });
    // A paper space view of an unknown layout stays a paper space view.
    d.views.push(NamedView { name: "Loose".into(), center: Vec2::ZERO, height: 1.0, width: 1.0, layout: Some(String::new()), ..Default::default() });
    let r = round_trip(&d);
    assert_eq!(r.views.len(), 3);
    for (a, b) in d.views.iter().zip(&r.views) {
        assert_eq!(
            (&a.name, a.center, a.height, a.width, &a.layer_state, &a.layout),
            (&b.name, b.center, b.height, b.width, &b.layer_state, &b.layout)
        );
        assert!((a.twist - b.twist).abs() < 1e-12, "{}: {} vs {}", a.name, a.twist, b.twist);
    }
}

/// Another program's paper space view (flag 70 = 1, no CADCraft xdata) and a hostile twist.
#[test]
fn foreign_paper_view_and_hostile_twist() {
    let tags: &[(i32, &str)] = &[
        (0, "SECTION"),
        (2, "TABLES"),
        (0, "TABLE"),
        (2, "VIEW"),
        (0, "VIEW"),
        (2, "PaperOne"),
        (70, "1"),
        (40, "10"),
        (41, "16"),
        (10, "3"),
        (20, "4"),
        (50, "90"),
        (0, "VIEW"),
        (2, "Odd"),
        (70, "0"),
        (40, "2"),
        (41, "2"),
        (50, "nan"),
        (0, "ENDTAB"),
        (0, "ENDSEC"),
        (0, "EOF"),
    ];
    let dxf: String = tags.iter().map(|(c, v)| format!("{c}\n{v}\n")).collect();
    let d = read_dxf(dxf.as_bytes()).expect("read");
    let paper = d.views.iter().find(|v| v.name == "PaperOne").expect("PaperOne");
    assert_eq!(paper.layout.as_deref(), Some(""));
    assert!((paper.twist - std::f64::consts::FRAC_PI_2).abs() < 1e-12);
    let odd = d.views.iter().find(|v| v.name == "Odd").expect("Odd");
    assert_eq!((odd.twist, odd.layout.as_deref()), (0.0, None));
    let again = round_trip(&d);
    assert_eq!(again.views, d.views);
}
