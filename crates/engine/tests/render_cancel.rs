//! B4-019: a running RENDER stops within a sample pass of a cancel request (MCP
//! `notifications/cancelled` or the `cancel` tool), and `samples` has a documented maximum.
//! In a file of its own: the cancel request is process-wide.

use cadaddy_engine::Session;
use cadaddy_engine::doc::{Common, EntityKind, Mesh, MeshFace, MeshKind, Space};
use cadaddy_engine::geom::Vec3;
use serde_json::json;

fn session_with_box() -> Session {
    let mut s = Session::new();
    s.viewport_px = (320.0, 200.0);
    let v = |x: f64, y: f64, z: f64| Vec3::new(x, y, z);
    let vertices = vec![
        v(0.0, 0.0, 0.0),
        v(4.0, 0.0, 0.0),
        v(4.0, 4.0, 0.0),
        v(0.0, 4.0, 0.0),
        v(0.0, 0.0, 2.0),
        v(4.0, 0.0, 2.0),
        v(4.0, 4.0, 2.0),
        v(0.0, 4.0, 2.0),
    ];
    let f = |a: [u32; 4]| MeshFace { v: a.to_vec(), hidden: 0 };
    let faces = vec![f([0, 3, 2, 1]), f([4, 5, 6, 7]), f([0, 1, 5, 4]), f([1, 2, 6, 5]), f([2, 3, 7, 6]), f([3, 0, 4, 7])];
    let st = s.state_mut().unwrap();
    std::sync::Arc::make_mut(&mut st.doc)
        .add(&Space::Model, Common::default(), EntityKind::Mesh(Mesh { vertices, faces, kind: MeshKind::Subd }))
        .unwrap();
    s.execute("view.seiso", &json!({})).unwrap();
    s.execute("zoom.extents", &json!({})).unwrap();
    s
}

#[test]
fn a_running_render_stops_when_cancelled() {
    let mut s = session_with_box();
    let canceller = std::thread::spawn(|| {
        // Let the render start, then ask it to stop (as the MCP reader thread does).
        std::thread::sleep(std::time::Duration::from_millis(400));
        cadaddy_engine::request_cancel();
    });
    let start = std::time::Instant::now();
    // At the largest sample count this runs for many minutes unless it is cancelled.
    let r = s.execute("render", &json!({ "width": 256, "height": 160, "samples": 65536, "preset": "draft" }));
    let took = start.elapsed();
    canceller.join().unwrap();
    let e = r.expect_err("the render was cancelled").to_string();
    assert!(e.contains("cancelled"), "{e}");
    assert!(took < std::time::Duration::from_secs(10), "took {took:?} after the cancel");
    // The next render is not cancelled by the old request.
    s.execute("render", &json!({ "width": 32, "height": 20, "samples": 1, "preset": "draft" })).unwrap();
}

#[test]
fn samples_has_a_documented_maximum() {
    let mut s = session_with_box();
    let spec = cadaddy_engine::find_command("render").unwrap();
    assert!(spec.params.contains("samples? (1 to 65536)"), "{}", spec.params);
    let e = s.execute("render", &json!({ "samples": 65537 })).unwrap_err().to_string();
    assert!(e.contains("65536"), "{e}");
}
