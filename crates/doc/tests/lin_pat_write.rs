//! Linetypes and hatch patterns written as `.lin` / `.pat` files read back the same (Q152: the
//! editors save user files).

use cadaddy_doc::library::files::{UserPattern, parse_lin, parse_pat, write_lin, write_pat};
use cadaddy_doc::library::{self, PatternLine};
use cadaddy_doc::{DashElement, Linetype};
use cadaddy_geom::Vec2;

fn same(a: &Linetype, b: &Linetype) {
    assert_eq!(a.name.to_ascii_uppercase(), b.name, "{a:?}");
    assert_eq!(a.description, b.description);
    assert_eq!(a.pattern.len(), b.pattern.len(), "{}: {:?} vs {:?}", a.name, a.pattern, b.pattern);
    for (x, y) in a.pattern.iter().zip(&b.pattern) {
        assert!((x.length - y.length).abs() < 1e-12, "{}", a.name);
        assert_eq!(x.text, y.text);
        assert_eq!(x.shape_name, y.shape_name);
        if x.text.is_some() || x.shape_name.is_some() {
            assert_eq!(x.style, y.style);
            assert!((x.scale - y.scale).abs() < 1e-12);
            assert!((x.rotation - y.rotation).abs() < 1e-9 && x.absolute == y.absolute);
            assert!((x.offset - y.offset).len() < 1e-12);
        }
    }
}

#[test]
fn linetypes_with_dashes_text_and_shapes_read_back() {
    let mut lts = library::standard_linetypes();
    lts.push(Linetype {
        name: "GAS_LINE".into(),
        description: "Gas line ---- GAS ---- GAS ----".into(),
        pattern: vec![
            DashElement::dash(0.5),
            DashElement {
                text: Some("GAS, [main]".into()),
                style: Some("Standard".into()),
                scale: 0.1,
                rotation: 15f64.to_radians(),
                offset: Vec2::new(-0.1, -0.05),
                ..DashElement::dash(-0.2)
            },
            DashElement {
                shape_name: Some("ARROW".into()),
                style: Some("cadaddy.shx".into()),
                scale: 0.2,
                absolute: true,
                rotation: 0.5,
                ..DashElement::dash(-0.25)
            },
        ],
    });
    let text = write_lin(&lts).unwrap();
    assert!(text.contains("*FENCELINE1,Fenceline circle") && text.contains("[CIRC,cadaddy.shx,S=0.1]"), "{text}");
    let back = parse_lin(&text);
    assert!(back.warnings.is_empty(), "{:?}\n{text}", back.warnings);
    assert_eq!(back.items.len(), lts.len());
    for (a, b) in lts.iter().zip(&back.items) {
        same(a, b);
    }
    // A shape with no name (its file wasn't found) can't be written; a name that would break the
    // file is refused.
    let mut nameless = lts[lts.len() - 1].clone();
    nameless.pattern[2].shape_name = None;
    nameless.pattern[2].shape = Some(7);
    assert!(write_lin(&[nameless]).is_err());
    assert!(write_lin(&[Linetype::simple("A,B", "", &[1.0, -1.0])]).is_err());
}

#[test]
fn hatch_patterns_read_back() {
    let pats = vec![
        UserPattern {
            name: "BRICKS_A".into(),
            description: "Running bond, 3 x 8".into(),
            lines: vec![
                PatternLine { angle: 0.0, origin: (0.0, 0.0), delta: (0.0, 3.0), dashes: vec![] },
                PatternLine { angle: 90.0, origin: (0.0, 0.0), delta: (3.0, 4.0), dashes: vec![3.0, -3.0] },
            ],
        },
        UserPattern {
            name: "DIAG".into(),
            description: String::new(),
            lines: vec![PatternLine { angle: 45.0, origin: (0.5, -0.25), delta: (0.0, 0.125), dashes: vec![0.25, -0.125, 0.0, -0.125] }],
        },
    ];
    let text = write_pat(&pats).unwrap();
    let back = parse_pat(&text);
    assert!(back.warnings.is_empty(), "{:?}", back.warnings);
    // The description's comma can't stay (it would end the description early).
    assert_eq!(back.items[0].description, "Running bond  3 x 8");
    assert_eq!(back.items[0].lines, pats[0].lines);
    assert_eq!(back.items[1], pats[1]);
    let bad = UserPattern {
        name: "NAN".into(),
        description: String::new(),
        lines: vec![PatternLine { angle: f64::NAN, origin: (0.0, 0.0), delta: (0.0, 1.0), dashes: vec![] }],
    };
    assert!(write_pat(&[bad]).is_err());
}
