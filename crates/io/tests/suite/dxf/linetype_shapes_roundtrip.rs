//! Linetypes with shapes (Q152) are kept in DXF and DWG: the shape element, its file (CADaddy's
//! built-in `cadaddy.shx` as an unnamed shape style), size, offsets and rotation; reopened, the
//! shapes have their names again and draw.

use cadaddy_doc::{Common, DashElement, Drawing, EntityKind, Line, Linetype, Space, library};
use cadaddy_geom::{Vec2, Vec3};

fn with_shapes() -> Drawing {
    let mut d = Drawing::new_imperial();
    for n in ["FENCELINE2", "TRACKS", "BATTING"] {
        d.linetypes.push(library::standard_linetypes().into_iter().find(|l| l.name == n).unwrap());
    }
    // A shape with an offset and an absolute rotation, given only by name (as a .lin file does).
    d.linetypes.push(Linetype {
        name: "FLOW".into(),
        description: "Flow ---->---->".into(),
        pattern: vec![
            DashElement::dash(0.5),
            DashElement {
                shape_name: Some("ARROW".into()),
                style: Some("cadaddy.shx".into()),
                scale: 0.2,
                rotation: 0.3,
                absolute: true,
                offset: Vec2::new(0.05, -0.02),
                ..DashElement::dash(-0.2)
            },
        ],
    });
    for (i, n) in ["FENCELINE2", "TRACKS", "BATTING", "FLOW"].iter().enumerate() {
        let y = i as f64;
        let common = Common { linetype: (*n).into(), ..Default::default() };
        d.add(&Space::Model, common, EntityKind::Line(Line { a: Vec3::new(0.0, y, 0.0), b: Vec3::new(10.0, y, 0.0) })).unwrap();
    }
    d
}

fn shapes(d: &Drawing, name: &str) -> Vec<DashElement> {
    d.linetype(name).unwrap_or_else(|| panic!("{name}")).pattern.iter().filter(|e| e.shape.is_some()).cloned().collect()
}

fn drawn(d: &Drawing) -> usize {
    cadaddy_render::build(d, &Space::Model, &cadaddy_render::Options::default()).prims.len()
}

#[test]
fn shape_linetypes_survive_dxf_and_dwg() {
    let d = with_shapes();
    let before = drawn(&d);
    for name in ["round.dxf", "round.dwg"] {
        let bytes = cadaddy_io::write(&d, name).unwrap();
        let back = cadaddy_io::read(&bytes, name).unwrap();
        for lt in ["FENCELINE2", "TRACKS", "BATTING"] {
            let (a, b) = (shapes(&d, lt), shapes(&back, lt));
            assert_eq!(b.len(), 1, "{name} {lt}: {:?}", back.linetype(lt).map(|l| &l.pattern));
            assert_eq!(b[0].shape, a[0].shape, "{name} {lt}");
            assert_eq!(b[0].shape_name, a[0].shape_name, "{name} {lt}: the name from the shape file");
            assert!((b[0].scale - a[0].scale).abs() < 1e-9, "{name} {lt}");
            assert!(b[0].style.as_deref().is_some_and(|s| s.eq_ignore_ascii_case("cadaddy.shx")), "{name} {lt}: {:?}", b[0].style);
        }
        // The arrow given by name went out by number and came back with both.
        let f = shapes(&back, "FLOW");
        assert_eq!((f.len(), f[0].shape, f[0].shape_name.as_deref()), (1, Some(6), Some("ARROW")), "{name}");
        assert!(f[0].absolute && (f[0].rotation - 0.3).abs() < 1e-9 && (f[0].offset - Vec2::new(0.05, -0.02)).len() < 1e-9, "{name}: {:?}", f[0]);
        // And they draw the same.
        assert_eq!(drawn(&back), before, "{name}");
        // A linetype read back can be written to a .lin file again.
        let lts: Vec<Linetype> = ["FENCELINE2", "FLOW"].iter().map(|n| back.linetype(n).unwrap().clone()).collect();
        let lin = cadaddy_doc::library::files::write_lin(&lts).unwrap();
        assert!(lin.contains("[BOX,") && lin.contains("[ARROW,"), "{name}: {lin}");
    }
}
