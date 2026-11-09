//! End to end: Walk on and off through its menu command (the Walk panel shows, then the drawing
//! again), viewpoints and a camera tour saved, and the walkthrough published as a .cdywalk; the
//! tour exported as video frames (`ui.walk.export`) and, with ffmpeg, an MP4.

use crate::e2e_common::{E2e, folder};
use serde_json::json;

#[test]
fn walk_viewpoints_tour_and_publish() {
    let mut e = E2e::new();
    let dir = folder("walktours");
    e.exec("layer.new", json!({ "name": "A-WALL", "current": true }));
    e.exec("pline", json!({ "vertices": [[0, 0], [240, 0], [240, 120], [0, 120]], "closed": true }));
    e.exec("pline", json!({ "vertices": [[6, 6], [234, 6], [234, 114], [6, 114]], "closed": true }));
    e.exec("saveas", json!({ "path": dir.join("Sample Plan.dxf") }));
    // Walk on: its panel is on screen; off: back to the drawing.
    e.menu("ui.walk");
    assert!(e.shows("Back to the drawing (Esc)"), "the Walk panel");
    e.menu("ui.walk");
    assert!(!e.shows("Back to the drawing (Esc)"));
    // Viewpoints and a tour between them.
    e.exec("tour.view", json!({ "name": "Door", "eye": [2, 2, 5.5], "yaw": 45 }));
    e.exec("tour.view", json!({ "name": "Corner", "eye": [18, 8, 5.5], "yaw": 225 }));
    let t = e.exec("tour.create", json!({ "name": "Project A Tour", "stops": ["Door", "Corner"] }));
    assert!(t["duration"].as_f64().unwrap() > 0.0, "{t}");
    let l = e.exec("tour.list", json!({}));
    assert_eq!((l["views"].as_array().unwrap().len(), l["tours"][0]["name"].as_str()), (2, Some("Project A Tour")));
    // Published as a walkthrough package.
    let out = dir.join("Sample Plan.cdywalk");
    let p = e.exec("walk.publish", json!({ "path": out }));
    assert!(std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0) > 0, "{p}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_tour_exports_its_frames_and_video() {
    let mut e = E2e::new();
    let dir = folder("walkexport");
    e.exec("layer.new", json!({ "name": "A-WALL", "current": true }));
    e.exec("pline", json!({ "vertices": [[0, 0], [240, 0], [240, 120], [0, 120]], "closed": true }));
    e.exec("pline", json!({ "vertices": [[6, 6], [234, 6], [234, 114], [6, 114]], "closed": true }));
    e.exec("saveas", json!({ "path": dir.join("Sample Plan.dxf") }));
    e.exec("tour.view", json!({ "name": "Door", "eye": [2, 2, 5.5], "yaw": 45 }));
    e.exec("tour.view", json!({ "name": "Corner", "eye": [18, 8, 5.5], "yaw": 225 }));
    let t = e.exec("tour.create", json!({ "name": "Project A Tour", "stops": ["Door", "Corner"] }));
    let duration = t["duration"].as_f64().unwrap();

    let frames = dir.join("frames");
    let mp4 = dir.join("tour.mp4");
    let fps = 4.0;
    let r = e.exec("ui.walk.export", json!({ "tour": "Project A Tour", "folder": frames, "fps": fps, "width": 160, "height": 90, "mp4": mp4 }));
    // One frame per tick of the tour, each a file.
    let n = r["frames"].as_u64().unwrap() as f64;
    assert!((n - fps * duration).abs() <= 1.0, "{n} frames for {duration} s at {fps} fps: {r}");
    let files = std::fs::read_dir(&frames).unwrap().filter(|f| f.as_ref().is_ok_and(|f| f.path().extension().is_some_and(|x| x == "png"))).count();
    assert_eq!(files as f64, n, "{r}");
    // The first and last frames show the room, not one flat colour.
    for k in ["first", "last"] {
        let img = image::open(frames.join(r[k].as_str().unwrap())).unwrap().to_rgba8();
        assert_eq!((img.width(), img.height()), (160, 90), "{r}");
        let shown = crate::e2e_common::inked(&img);
        assert!(shown * 100 > (160 * 90) as usize, "{k} frame: {shown} pixels off its background");
    }
    // The video: made by ffmpeg, or its command given with the reason.
    match r.get("mp4") {
        Some(_) => assert!(std::fs::metadata(&mp4).map(|m| m.len()).unwrap_or(0) > 0, "{r}"),
        None => {
            assert!(r["message"].as_str().unwrap().contains("ffmpeg isn't installed"), "{r}");
            assert!(r["ffmpeg"].as_str().unwrap().starts_with("ffmpeg "), "{r}");
        }
    }
    // A tour that isn't there is refused.
    assert!(e.call("engine.execute", json!({ "command": "ui.walk.export", "params": { "tour": "Nope", "folder": frames } })).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}
