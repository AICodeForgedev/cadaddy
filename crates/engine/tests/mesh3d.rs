//! 3D objects in the engine (Q50): meshes move, explode into 3D faces, are picked inside their
//! faces and fit 3D views; VSCURRENT sets the visual style; a shaded or 3D view exports and plots
//! as a shaded raster with hidden lines hidden.

use cadaddy_engine::Session;
use cadaddy_engine::doc::{Common, EntityKind, Handle, Mesh, MeshFace, MeshKind, Space};
use cadaddy_engine::geom::{Vec2, Vec3};
use serde_json::json;

/// A 10 × 6 × 4 box mesh at the origin.
fn box_mesh() -> Mesh {
    let v = |x: f64, y: f64, z: f64| Vec3::new(x, y, z);
    let vertices = vec![
        v(0.0, 0.0, 0.0),
        v(10.0, 0.0, 0.0),
        v(10.0, 6.0, 0.0),
        v(0.0, 6.0, 0.0),
        v(0.0, 0.0, 4.0),
        v(10.0, 0.0, 4.0),
        v(10.0, 6.0, 4.0),
        v(0.0, 6.0, 4.0),
    ];
    let f = |a: [u32; 4]| MeshFace { v: a.to_vec(), hidden: 0 };
    let faces = vec![f([0, 3, 2, 1]), f([4, 5, 6, 7]), f([0, 1, 5, 4]), f([1, 2, 6, 5]), f([2, 3, 7, 6]), f([3, 0, 4, 7])];
    Mesh { vertices, faces, kind: MeshKind::Subd }
}

fn session_with_box() -> (Session, Handle) {
    let mut s = Session::new();
    s.viewport_px = (1200.0, 800.0);
    let h = s.add_entity(EntityKind::Mesh(box_mesh())).unwrap();
    (s, h)
}

fn mesh(s: &Session, h: Handle) -> Mesh {
    match &s.doc().unwrap().entity(h).unwrap().kind {
        EntityKind::Mesh(m) => m.clone(),
        k => panic!("{k:?}"),
    }
}

#[test]
fn meshes_move_explode_and_are_picked_inside_their_faces() {
    let (mut s, h) = session_with_box();
    s.execute("move", &json!({ "handles": [h.hex()], "delta": [1, 2] })).unwrap();
    assert_eq!(mesh(&s, h).vertices[6], Vec3::new(11.0, 8.0, 4.0));
    // Inside the top face (seen from above), well away from any edge.
    let d = s.doc().unwrap();
    assert_eq!(cadaddy_engine::select::pick(d, &Space::Model, Vec2::new(6.0, 5.0), 0.1), Some(h));
    assert_eq!(cadaddy_engine::select::pick(d, &Space::Model, Vec2::new(30.0, 30.0), 0.1), None);
    // LIST / properties name it; EXPLODE gives its six faces.
    assert_eq!(s.doc().unwrap().entity(h).unwrap().kind.type_name(), "Mesh");
    let r = s.execute("explode", &json!({ "handles": [h.hex()] })).unwrap();
    let faces = s.doc().unwrap().model.iter().filter(|e| matches!(e.kind, EntityKind::Face3d(_))).count();
    assert_eq!(faces, 6, "{r}");
    assert!(s.doc().unwrap().entity(h).is_none());
}

#[test]
fn a_3d_view_fits_the_mesh() {
    let (mut s, _) = session_with_box();
    s.execute("view.front", &json!({})).unwrap();
    // Seen from the front, the box is 10 wide and 4 tall: the view holds all of its height.
    let v = s.state().unwrap().view();
    assert!(v.height >= 4.0 && v.height < 20.0, "{v:?}");
}

#[test]
fn vscurrent_sets_the_style() {
    let (mut s, _) = session_with_box();
    assert_eq!(s.state().unwrap().visual_style.name(), "2D Wireframe");
    s.execute("vscurrent", &json!({ "style": "shaded with edges" })).unwrap();
    assert_eq!(s.state().unwrap().visual_style.name(), "Shaded with Edges");
    s.execute("vs.hidden", &json!({})).unwrap();
    assert_eq!(s.state().unwrap().visual_style.name(), "Hidden");
    for l in ["vscurrent", "r"] {
        s.cmdline(l).unwrap();
    }
    assert_eq!(s.state().unwrap().visual_style.name(), "Realistic");
    assert!(s.execute("vscurrent", &json!({ "style": "sketchy" })).is_err());
}

fn png_pixels(png: &[u8]) -> (u32, u32, Vec<u8>) {
    let r = cadaddy_engine::render::images::decode(png).expect("a PNG");
    (r.width, r.height, r.rgba)
}

#[test]
fn shaded_views_export_and_plot_as_rasters() {
    let (mut s, _) = session_with_box();
    // A line behind the box (seen from the front): hidden in Hidden, seen in Wireframe.
    s.add_entity(EntityKind::Line(cadaddy_engine::doc::Line { a: Vec3::new(-5.0, 20.0, 2.0), b: Vec3::new(15.0, 20.0, 2.0) })).unwrap();
    // The plan view in 2D Wireframe: no shaded output.
    assert!(cadaddy_engine::shaded_png(&s, 200, 100).is_none());
    s.execute("view.front", &json!({})).unwrap();
    let count_dark = |png: &[u8]| {
        let (_, _, px) = png_pixels(png);
        px.as_chunks::<4>().0.iter().filter(|p| p[0] < 128 && p[1] < 128 && p[2] < 128).count()
    };
    s.execute("vscurrent", &json!({ "style": "hidden" })).unwrap();
    let hidden = cadaddy_engine::shaded_png(&s, 400, 200).expect("a shaded export");
    s.execute("vscurrent", &json!({ "style": "wireframe" })).unwrap();
    let wire = cadaddy_engine::shaded_png(&s, 400, 200).unwrap();
    // The line behind the box shows through in Wireframe only: more ink.
    assert!(count_dark(&wire) > count_dark(&hidden), "{} vs {}", count_dark(&wire), count_dark(&hidden));
    // Shaded: grey faces on white.
    s.execute("vscurrent", &json!({ "style": "shaded" })).unwrap();
    let (w, h, px) = png_pixels(&cadaddy_engine::shaded_png(&s, 400, 200).unwrap());
    let mid = ((h / 2 * w + w / 2) * 4) as usize;
    assert!(px[mid] < 250 && px[mid] > 20, "the box's front face is lit grey: {:?}", &px[mid..mid + 4]);
}

#[test]
fn a_shaded_view_plots_as_displayed() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: Some(|d, sp, o| cadaddy_io::plotter::plot(d, sp, o).map_err(|e| e.to_string())),
        read_pdf: None,
    });
    let (mut s, _) = session_with_box();
    s.execute("view.swiso", &json!({})).unwrap();
    s.execute("vscurrent", &json!({ "style": "realistic" })).unwrap();
    let dir = std::env::temp_dir().join(format!("cadaddy-mesh3d-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let pdf = dir.join("box.pdf");
    s.execute("plot", &json!({ "path": pdf.to_string_lossy() })).unwrap();
    let bytes = std::fs::read(&pdf).unwrap();
    assert!(bytes.starts_with(b"%PDF") && String::from_utf8_lossy(&bytes).contains("/Subtype /Image"), "a page holding the shaded raster");
    let png = dir.join("box.png");
    s.execute("plot", &json!({ "path": png.to_string_lossy(), "output": "png", "resolution": 50 })).unwrap();
    let (_, _, px) = png_pixels(&std::fs::read(&png).unwrap());
    assert!(px.as_chunks::<4>().0.iter().any(|p| p[0] < 240 && p[0] > 30 && p[0] == p[1]), "grey lit faces");
    // `shade: false` plots the plan's vectors as before.
    s.execute("plot", &json!({ "path": pdf.to_string_lossy(), "shade": false })).unwrap();
    assert!(!String::from_utf8_lossy(&std::fs::read(&pdf).unwrap()).contains("/Subtype /Image"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn mesh_vertices_and_edges_snap() {
    let (s, _) = session_with_box();
    let d = s.doc().unwrap();
    use cadaddy_engine::snap::{mode, osnap};
    let end = osnap(d, &Space::Model, Vec2::new(9.9, 5.9), 0.5, mode::END, None, false).expect("a corner");
    assert!(end.point.near(Vec2::new(10.0, 6.0), 1e-9));
    let mid = osnap(d, &Space::Model, Vec2::new(5.1, 0.1), 0.5, mode::MID, None, false).expect("an edge's midpoint");
    assert!(mid.point.near(Vec2::new(5.0, 0.0), 1e-9));
    let _ = Common::default();
}
