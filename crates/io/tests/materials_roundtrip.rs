//! Materials through DXF and DWG: the drawing's MATERIAL objects (library recipes, texture files,
//! physically based values, mapping), layers' attached materials and objects' assigned ones
//! (group 347), objects' own texture mapping.

use cadaddy_doc::material::{self, Mapping, Material, Projection, Texture};
use cadaddy_doc::{Common, Drawing, EntityKind, Layer, Line, Space};
use cadaddy_geom::Vec3;

fn sample() -> Drawing {
    let mut d = Drawing::new_metric();
    let brick = material::recipe("brick-red").unwrap().material(0.001);
    let photo = Material {
        name: "Site photo floor".into(),
        description: "from a scan".into(),
        color: cadaddy_doc::color::Rgb(120, 110, 90),
        color_map: Some(Texture::File("textures/floor.jpg".into())),
        normal_map: Some(Texture::File("textures/floor_n.png".into())),
        roughness: 0.62,
        metallic: 0.1,
        normal_strength: 1.4,
        mapping: Mapping { projection: Projection::Box, size_u: 900.0, size_v: 600.0, offset_u: 10.0, offset_v: -5.0, rotation: 0.25 },
        ..Material::default()
    };
    let glass = material::recipe("glass-clear").unwrap().material(0.001);
    d.materials = vec![brick.clone(), photo.clone(), glass];
    d.layers.push(Layer { name: "Walls".into(), material: Some(brick.name.clone()), ..Layer::default() });
    let line = |a: f64| EntityKind::Line(Line { a: Vec3::new(a, 0.0, 0.0), b: Vec3::new(a, 10.0, 0.0) });
    d.add(&Space::Model, Common { layer: "Walls".into(), ..Common::default() }, line(0.0)).unwrap();
    d.add(&Space::Model, Common { material: Some(photo.name.clone()), ..Common::default() }, line(1.0)).unwrap();
    let mut by_block = Common { material: Some("ByBlock".into()), ..Common::default() };
    material::set_object_mapping(
        &mut by_block.xdata,
        Some(&Mapping { projection: Projection::Cylindrical, size_u: 300.0, size_v: 150.0, ..Mapping::default() }),
    );
    d.add(&Space::Model, by_block, line(2.0)).unwrap();
    d
}

fn materials_of(d: &Drawing) -> Vec<(Option<String>, Option<Mapping>)> {
    d.model.iter().map(|e| (e.common.material.clone(), material::object_mapping(&e.common.xdata))).collect()
}

fn close(a: &Material, b: &Material) {
    assert_eq!(
        (&a.name, &a.description, a.color, &a.color_map, &a.roughness_map, &a.normal_map),
        (&b.name, &b.description, b.color, &b.color_map, &b.roughness_map, &b.normal_map)
    );
    for (x, y) in [
        (a.roughness, b.roughness),
        (a.metallic, b.metallic),
        (a.opacity, b.opacity),
        (a.refraction, b.refraction),
        (a.normal_strength, b.normal_strength),
    ] {
        assert!((x - y).abs() < 1e-9, "{} {x} {y}", a.name);
    }
    let (m, n) = (a.mapping, b.mapping);
    assert_eq!(m.projection, n.projection, "{}", a.name);
    for (x, y) in [(m.size_u, n.size_u), (m.size_v, n.size_v), (m.offset_u, n.offset_u), (m.offset_v, n.offset_v), (m.rotation, n.rotation)] {
        assert!((x - y).abs() < 1e-6, "{} mapping {x} {y}", a.name);
    }
}

#[test]
fn materials_survive_dxf_and_dwg() {
    let d = sample();
    for name in ["x.dxf", "x.dwg"] {
        let bytes = cadaddy_io::write(&d, name).unwrap();
        let back = cadaddy_io::read(&bytes, name).unwrap();
        assert_eq!(back.materials.len(), d.materials.len(), "{name}: {:?}", back.materials.iter().map(|m| &m.name).collect::<Vec<_>>());
        for (a, b) in back.materials.iter().zip(&d.materials) {
            close(a, b);
        }
        assert_eq!(back.layer("Walls").and_then(|l| l.material.clone()), Some("Brick, red, running bond".into()), "{name}");
        assert_eq!(back.layer("0").and_then(|l| l.material.clone()), None, "{name}: Global");
        assert_eq!(materials_of(&back), materials_of(&d), "{name}");
        // Read again and written again: stable, nothing doubled.
        let again = cadaddy_io::read(&cadaddy_io::write(&back, name).unwrap(), name).unwrap();
        assert_eq!(again.materials.len(), d.materials.len(), "{name}");
        assert_eq!(materials_of(&again), materials_of(&d), "{name}");
    }
}

#[test]
fn other_programs_see_standard_material_groups() {
    let text = String::from_utf8(cadaddy_io::write(&sample(), "x.dxf").unwrap()).unwrap();
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    let count = |name: &str| lines.windows(3).filter(|w| w[0] == "AcDbMaterial" && w[1] == "1" && w[2] == name).count();
    for n in ["Global", "ByLayer", "ByBlock", "Site photo floor", "Glass, clear"] {
        assert_eq!(count(n), 1, "{n}");
    }
    // The texture file and the dictionary entry, as the DXF Reference gives them.
    assert!(lines.windows(2).any(|w| w[0] == "3" && w[1] == "textures/floor.jpg"));
    assert!(lines.windows(2).any(|w| w[0] == "3" && w[1] == "ACAD_MATERIAL"));
    assert!(lines.windows(2).any(|w| w[0] == "1001" && w[1] == material::MAP_APP), "the object's mapping");
    assert!(lines.windows(4).any(|w| w[0] == "AcDbRegAppTableRecord" && w[1] == "2" && w[2] == material::MAP_APP), "registered");
}

#[test]
fn hostile_material_objects_are_safe() {
    // A MATERIAL with nonsense values and a 347 pointing nowhere.
    let dxf = "0\nSECTION\n2\nENTITIES\n0\nLINE\n8\n0\n100\nAcDbEntity\n347\nFFFF\n10\n0\n20\n0\n11\n1\n21\n1\n0\nENDSEC\n0\nSECTION\n2\nOBJECTS\n0\nDICTIONARY\n5\nC\n3\nACAD_MATERIAL\n350\nD\n0\nDICTIONARY\n5\nD\n3\nX\n350\nE\n0\nMATERIAL\n5\nE\n100\nAcDbMaterial\n1\nX\n44\nnan\n140\n-9\n145\n1e308\n73\n77\n43\n0\n43\n0\n0\nENDSEC\n0\nEOF\n";
    let d = cadaddy_io::read(dxf.as_bytes(), "h.dxf").unwrap();
    let m = d.material("X").unwrap();
    assert!((0.0..=1.0).contains(&m.roughness) && (0.0..=1.0).contains(&m.opacity) && m.refraction <= 5.0);
    assert_eq!(m.mapping, Mapping::default());
    assert_eq!(d.model.iter().next().unwrap().common.material, None, "a 347 to nothing is ByLayer");
}
