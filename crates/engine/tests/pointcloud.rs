//! Point clouds in the drawing: attached under the model from a LAS file, its octree's
//! level-of-detail counts, clipping, colour maps, snapping to its points, drawn by the renderer,
//! and kept through DXF.

use cadaddy_engine::Session;
use cadaddy_engine::doc::pointcloud::{Cloud, ColorMode};
use cadaddy_engine::doc::{EntityKind, Space};
use cadaddy_engine::geom::Vec2;
use serde_json::json;

fn hooks() {
    cadaddy_engine::cmd::pointcloud::set_reader(cadaddy_io::pointcloud_io::read);
}

/// A 100 × 100 grid of points 1 apart at z = (x + y) / 10, coloured by x.
fn scan(dir: &std::path::Path) -> String {
    let mut c = Cloud::default();
    for i in 0..100 {
        for j in 0..100 {
            c.points.push([i as f32, j as f32, (i + j) as f32 / 10.0]);
            c.colors.push([(i * 2) as u8, 0, 0]);
            c.intensity.push((j * 600) as u16);
        }
    }
    let p = dir.join("site.las");
    std::fs::write(&p, cadaddy_io::pointcloud_io::write_las(&c)).unwrap();
    p.to_string_lossy().into_owned()
}

fn folder(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-pc-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn attached_counted_clipped_coloured_and_snapped() {
    hooks();
    let dir = folder("attach");
    let path = scan(&dir);
    let mut s = Session::new();
    s.execute("line", &json!({ "points": [[0, 0], [10, 0]] })).unwrap();
    let r = s.execute("pointcloudattach", &json!({ "path": path, "at": [1000, 0, 0], "scale": 2 })).unwrap();
    assert_eq!(r["points"], 10_000);
    let h = r["handle"].as_str().unwrap().to_string();
    let d = s.doc().unwrap();
    assert!(matches!(d.model.iter().next().map(|e| &e.kind), Some(EntityKind::PointCloud(_))), "under the model: drawn first");
    let ext = d.extents(&Space::Model);
    assert!(ext.max.x >= 1000.0 + 99.0 * 2.0 - 0.01, "{ext:?}");

    // The octree's level-of-detail counts.
    let info = s.execute("pointcloud.info", &json!({ "handle": h, "budget": 1000 })).unwrap();
    let levels: Vec<u64> = info["levels"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap()).collect();
    assert_eq!(levels.first(), Some(&1));
    assert_eq!(levels.last(), Some(&10_000));
    let lod = info["lod"].as_u64().unwrap();
    assert!((500..=2000).contains(&lod), "{lod}");
    assert!(info["colors"] == true && info["intensity"] == true);

    // Clipping to a box in drawing coordinates: x 1000..1040 (cloud 0..20), all y and z.
    s.execute("pointcloudclip", &json!({ "handle": h, "box": [[1000, -1, -10], [1040, 1000, 100]] })).unwrap();
    let shown = s.execute("pointcloud.info", &json!({ "handle": h })).unwrap()["shown"].as_u64().unwrap();
    assert_eq!(shown, 21 * 100);
    s.execute("pointcloudclip", &json!({ "handle": h, "off": true })).unwrap();
    assert_eq!(s.execute("pointcloud.info", &json!({ "handle": h })).unwrap()["shown"], 10_000);

    s.execute("pointcloudcolormap", &json!({ "handle": h, "mode": "elevation" })).unwrap();
    let EntityKind::PointCloud(pc) = &s.doc().unwrap().model.iter().next().unwrap().kind else { panic!() };
    assert_eq!(pc.color_mode, ColorMode::Elevation);
    assert!(s.execute("pointcloudcolormap", &json!({ "handle": h, "mode": "plaid" })).is_err());

    // Node snap onto a point of the cloud: cloud (10, 20) is drawing (1020, 40).
    let hit =
        cadaddy_engine::snap::osnap(s.doc().unwrap(), &Space::Model, Vec2::new(1020.3, 40.2), 1.0, cadaddy_engine::snap::mode::NOD, None, false)
            .unwrap();
    assert!((hit.point - Vec2::new(1020.0, 40.0)).len() < 1e-6, "{:?}", hit.point);

    // The renderer draws its points (about the budget).
    let list = cadaddy_render::build(s.doc().unwrap(), &Space::Model, &cadaddy_render::Options::default());
    let points = list.prims.iter().filter(|p| matches!(p.kind, cadaddy_render::Kind::Point)).count();
    assert!(points >= 10_000, "{points} points drawn");

    // Bad attaches.
    assert!(s.execute("pointcloudattach", &json!({ "path": dir.join("missing.las").to_string_lossy() })).is_err());
    assert!(s.execute("pointcloud.info", &json!({ "handle": "1" })).is_err());
    // One undo per command.
    s.execute("undo", &json!({})).unwrap();
    let EntityKind::PointCloud(pc) = &s.doc().unwrap().model.iter().next().unwrap().kind else { panic!() };
    assert_eq!(pc.color_mode, ColorMode::Rgb);
}

#[test]
fn a_point_cloud_is_kept_through_dxf() {
    hooks();
    let dir = folder("dxf");
    let path = scan(&dir);
    let mut s = Session::new();
    let r = s.execute("pointcloudattach", &json!({ "path": path, "at": [5, 6, 7], "rotation": 30 })).unwrap();
    let h = r["handle"].as_str().unwrap().to_string();
    s.execute("pointcloudclip", &json!({ "handle": h, "box": [[0, 0, 0], [50, 50, 50]] })).unwrap();
    let before = s.doc().unwrap().model.iter().find_map(|e| if let EntityKind::PointCloud(pc) = &e.kind { Some(pc.clone()) } else { None }).unwrap();
    let bytes = cadaddy_io::write(s.doc().unwrap(), "x.dxf").unwrap();
    let back = cadaddy_io::read(&bytes, "x.dxf").unwrap();
    let after =
        back.model.iter().find_map(|e| if let EntityKind::PointCloud(pc) = &e.kind { Some(pc.clone()) } else { None }).expect("the point cloud");
    assert_eq!(after.path, before.path);
    assert!(after.clip.is_some());
    assert!((after.rotation - before.rotation).abs() < 1e-12 && after.insert == before.insert && after.count == 10_000);
}
