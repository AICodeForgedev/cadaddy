//! IFC4 and 3MF exchange: the WALLGEN building (walls, slab, ceiling, door, window), a room volume
//! and another mesh exported as IFC4 with storeys, read back by our own reader with the same counts
//! per class and the same geometry, imported as meshes on IFC-* layers; 3MF out.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::wallgen::made_as;
use cadaddy_engine::doc::{Common, Drawing, Entity, EntityKind, Handle, Line, LwPolyline, Mesh, MeshFace, RawTag, Space};
use cadaddy_engine::geom::Vec3;
use serde_json::json;

fn hooks() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
    });
}

/// Feet → a line in inches on A-WALL.
fn line(d: &mut Drawing, a: (f64, f64), b: (f64, f64)) {
    let p = |(x, y): (f64, f64)| Vec3 { x: x * 12.0, y: y * 12.0, z: 0.0 };
    let h = Handle(d.handseed.max(0x100));
    d.handseed = h.0 + 1;
    let mut e = Entity::new(h, EntityKind::Line(Line { a: p(a), b: p(b) }));
    e.common.layer = "A-WALL".into();
    d.model.push(e);
}

/// The two-room plan of the WALLGEN tests (a window gap and a doorway), built, plus a room
/// volume and a plain box.
fn building() -> Session {
    let mut d = Drawing::new_imperial();
    for (a, b) in [((0., 0.), (8., 0.)), ((12., 0.), (40., 0.)), ((40., 0.), (40., 10.)), ((40., 10.), (0., 10.)), ((0., 10.), (0., 0.))] {
        line(&mut d, a, b);
    }
    for (a, b) in
        [((0.5, 0.5), (8., 0.5)), ((12., 0.5), (39.5, 0.5)), ((39.5, 0.5), (39.5, 9.5)), ((39.5, 9.5), (0.5, 9.5)), ((0.5, 9.5), (0.5, 0.5))]
    {
        line(&mut d, a, b);
    }
    for x in [20.0, 20.5] {
        line(&mut d, (x, 0.5), (x, 3.0));
        line(&mut d, (x, 6.0), (x, 9.5));
    }
    let swing: Vec<_> = (0..=9)
        .map(|i| f64::from(i) * std::f64::consts::FRAC_PI_2 / 9.0)
        .map(|a| json!({"p": {"x": (20.0 - 3.0 * a.sin()) * 12.0, "y": (3.0 + 3.0 * a.cos()) * 12.0}, "bulge": 0.0, "start_width": 0.0, "end_width": 0.0}))
        .collect();
    let pl: LwPolyline = serde_json::from_value(json!({"vertices": swing, "closed": false})).unwrap();
    let h = Handle(d.handseed.max(0x100));
    d.handseed = h.0 + 1;
    let mut e = Entity::new(h, EntityKind::LwPolyline(pl));
    e.common.layer = "A-WALL".into();
    d.model.push(e);
    let mut s = Session::empty();
    s.open_drawing(d, "Plan", None);
    let rules = json!({
        "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 9.5, "ceiling_ft": 8, "base": [0, 0]}],
        "layers": [{"layer": "A-WALL", "kind": "wall", "skip_curves": true}]
    });
    s.execute("wallgen", &json!({ "rules": rules })).unwrap();
    // A room volume as ROOMS tags it, and a plain box.
    let cube = |x0: f64, y0: f64, s: f64, h: f64| Mesh {
        vertices: [(0., 0., 0.), (s, 0., 0.), (s, s, 0.), (0., s, 0.), (0., 0., h), (s, 0., h), (s, s, h), (0., s, h)]
            .iter()
            .map(|(x, y, z)| Vec3::new(x0 + x, y0 + y, *z))
            .collect(),
        faces: [[0, 3, 2, 1], [4, 5, 6, 7], [0, 1, 5, 4], [1, 2, 6, 5], [2, 3, 7, 6], [3, 0, 4, 7]]
            .iter()
            .map(|f| MeshFace { v: f.to_vec(), hidden: 0 })
            .collect(),
        ..Mesh::default()
    };
    let room = Common {
        layer: "3D-Rooms".into(),
        xdata: vec![
            RawTag { code: 1001, value: "CADADDY_ROOM".into() },
            RawTag { code: 1000, value: "Office".into() },
            RawTag { code: 1000, value: "101".into() },
        ],
        ..Common::default()
    };
    let d = s.doc_mut().unwrap();
    d.add(&Space::Model, room, EntityKind::Mesh(cube(12.0, 12.0, 96.0, 96.0))).unwrap();
    d.add(&Space::Model, Common::default(), EntityKind::Mesh(cube(600.0, 0.0, 24.0, 24.0))).unwrap();
    s
}

fn count(s: &Session, part: &str) -> usize {
    s.doc().unwrap().model.iter().filter(|e| made_as(e).as_deref() == Some(part)).count()
}

fn bounds(m: &Mesh) -> (Vec3, Vec3) {
    m.vertices.iter().fold((Vec3::new(f64::MAX, f64::MAX, f64::MAX), Vec3::new(f64::MIN, f64::MIN, f64::MIN)), |(lo, hi), v| {
        (Vec3::new(lo.x.min(v.x), lo.y.min(v.y), lo.z.min(v.z)), Vec3::new(hi.x.max(v.x), hi.y.max(v.y), hi.z.max(v.z)))
    })
}

#[test]
fn the_building_round_trips_through_ifc() {
    hooks();
    let mut s = building();
    let dir = std::env::temp_dir().join(format!("cadaddy-ifc-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Two Rooms.ifc");
    s.execute("export", &json!({ "path": path.to_string_lossy() })).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.starts_with("ISO-10303-21;") && text.contains("FILE_SCHEMA(('IFC4'));") && text.trim_end().ends_with("END-ISO-10303-21;"));
    assert!(text.contains("IFCPROJECT(") && text.contains("'Two Rooms'"), "the project is named after the file");
    assert_eq!(text.matches("=IFCBUILDINGSTOREY(").count(), 1);
    assert!(text.contains("'L1'"), "the storey is WALLGEN's level");
    assert!(text.contains("IFCRELCONTAINEDINSPATIALSTRUCTURE("));

    // Read back by our own reader: the same objects per class.
    let back = cadaddy_io::read(&std::fs::read(&path).unwrap(), "Two Rooms.ifc").unwrap();
    let on = |layer: &str| back.model.iter().filter(|e| e.common.layer == layer).count();
    assert_eq!(on("IFC-Wall"), count(&s, "wall"));
    assert_eq!(on("IFC-Slab"), count(&s, "floor"));
    assert_eq!(on("IFC-Covering"), count(&s, "ceiling"));
    assert_eq!((on("IFC-Door"), on("IFC-Window")), (1, 1));
    assert_eq!(on("IFC-Space"), 1);
    assert_eq!(on("IFC-BuildingElementProxy"), 1);
    assert!(count(&s, "wall") >= 4);
    // Geometry: every wall's extents, inches → metres.
    let mut want: Vec<(Vec3, Vec3)> = s
        .doc()
        .unwrap()
        .model
        .iter()
        .filter(|e| made_as(e).as_deref() == Some("wall"))
        .filter_map(|e| if let EntityKind::Mesh(m) = &e.kind { Some(bounds(m)) } else { None })
        .map(|(lo, hi)| (lo * 0.0254, hi * 0.0254))
        .collect();
    let mut got: Vec<(Vec3, Vec3)> = back
        .model
        .iter()
        .filter(|e| e.common.layer == "IFC-Wall")
        .filter_map(|e| if let EntityKind::Mesh(m) = &e.kind { Some(bounds(m)) } else { None })
        .collect();
    let key = |b: &(Vec3, Vec3)| (b.0.x * 1e4).round() as i64 * 1_000_000 + (b.0.y * 1e4).round() as i64;
    want.sort_by_key(key);
    got.sort_by_key(key);
    for (w, g) in want.iter().zip(&got) {
        for (a, b) in [(w.0, g.0), (w.1, g.1)] {
            assert!((a - b).len() < 1e-4, "{a:?} vs {b:?}");
        }
    }
    // Spaces keep their number and name.
    let space = back.model.iter().find(|e| e.common.layer == "IFC-Space").unwrap();
    let tag = cadaddy_io::ifc::ifc_tag(space).unwrap();
    assert_eq!(tag.0, "Space");
    assert!(text.contains("IFCSPACE(") && text.contains("'Office'") && text.contains("'101'"));

    // IMPORT puts them in a drawing on IFC-* layers.
    let mut t = Session::new();
    let r = t.execute("import", &json!({ "path": path.to_string_lossy() })).unwrap();
    assert!(t.doc().unwrap().layer("IFC-Wall").is_some(), "{r}");
    // The same drawing gives the same GlobalIds.
    let again = dir.join("again.ifc");
    s.execute("export", &json!({ "path": again.to_string_lossy() })).unwrap();
    let ids =
        |t: &str| t.lines().filter(|l| l.contains("IFCWALL(")).map(|l| l.split('\'').nth(1).unwrap_or_default().to_string()).collect::<Vec<_>>();
    assert_eq!(ids(&text), ids(&std::fs::read_to_string(&again).unwrap()));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn three_mf_holds_the_solids_and_meshes() {
    hooks();
    let mut s = building();
    let r = s.execute("export", &json!({ "format": "3mf" })).unwrap();
    let data = r["data"].as_str().unwrap();
    let bytes = cadaddy_engine::cmd::file::base64_decode(data).unwrap();
    assert!(bytes.starts_with(b"PK\x03\x04"));
    let entries = cadaddy_engine::cmd::file::zip::read(&bytes).unwrap();
    let names: Vec<&str> = entries.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["[Content_Types].xml", "_rels/.rels", "3D/3dmodel.model"]);
    let model = String::from_utf8_lossy(&entries[2].1);
    assert!(model.contains("unit=\"inch\""), "the drawing's unit");
    assert!(model.matches("<triangle ").count() > 50);
    // Nothing 3D: refused.
    assert!(Session::new().execute("export", &json!({ "format": "3mf" })).is_err());
    assert!(Session::new().execute("export", &json!({ "format": "ifc" })).is_err());
}
