//! A model saved with its drawing stays associative when the drawing is opened again (Q6378): what
//! plan it was built from and whether it follows the plan are saved with the drawing, so
//! `model.update` and `model.revise` work after reopening as before saving, without another
//! `model.build`.

use crate::helper::{hooks, scratch, serial};
use cadaddy_engine::Session;
use cadaddy_engine::cmd::wallgen::made_as;
use cadaddy_engine::doc::EntityKind;
use serde_json::{Value, json};

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("line", &json!({"points": [a, b]})).unwrap();
}

/// A made-up 40' x 20' storey of 6" walls (inches), split in two rooms, built with `build`.
fn plan(build: Value) -> Session {
    hooks();
    let mut s = Session::new();
    layer(&mut s, "A-WALL");
    for (x0, y0, x1, y1) in [(0.0, 0.0, 480.0, 240.0), (6.0, 6.0, 474.0, 234.0)] {
        line(&mut s, [x0, y0], [x1, y0]);
        line(&mut s, [x1, y0], [x1, y1]);
        line(&mut s, [x1, y1], [x0, y1]);
        line(&mut s, [x0, y1], [x0, y0]);
    }
    for x in [238.0, 244.0] {
        line(&mut s, [x, 6.0], [x, 234.0]);
    }
    s.execute("bspec.draft", &json!({"store": true})).unwrap();
    s.execute("model.build", &build).unwrap();
    s.set_selection(Vec::new());
    s
}

/// The east face of the built walls (largest x of any built wall vertex).
fn east(s: &Session) -> f64 {
    let mut x = f64::NEG_INFINITY;
    for e in s.doc().unwrap().model.iter().filter(|e| made_as(e).as_deref() == Some("wall")) {
        if let EntityKind::Mesh(m) = &e.kind {
            x = m.vertices.iter().map(|v| v.x).fold(x, f64::max);
        }
    }
    x
}

/// Move the plan's two east wall lines `dx` inches east.
fn stretch_east(s: &mut Session, dx: f64) {
    let handles: Vec<String> = s
        .doc()
        .unwrap()
        .model
        .iter()
        .filter(|e| made_as(e).is_none() && matches!(&e.kind, EntityKind::Line(l) if l.a.x > 470.0 && l.b.x > 470.0))
        .map(|e| e.handle.hex())
        .collect();
    assert_eq!(handles.len(), 2, "the plan's east wall lines");
    s.execute("move", &json!({"handles": handles, "from": [0, 0], "to": [dx, 0]})).unwrap();
}

fn reopen(path: &str) -> Session {
    let mut s = Session::new();
    s.execute("open", &json!({ "path": path })).unwrap();
    s
}

#[test]
fn an_associative_model_follows_its_plan_after_reopening() {
    let _turn = serial();
    let d = scratch("reopen-assoc");
    for name in ["m.cdy", "m.dxf"] {
        let mut s = plan(json!({}));
        let built_east = east(&s);
        assert!(built_east.is_finite(), "{name}: the model has walls");
        let path = d.join(name).to_string_lossy().into_owned();
        s.execute("saveas", &json!({ "path": path })).unwrap();
        drop(s);

        let mut s = reopen(&path);
        assert!((east(&s) - built_east).abs() < 1e-6, "{name}: the model opens as saved");
        // Nothing changed yet: not behind, and the plan matches what it was built from.
        let r = s.execute("model.update", &json!({})).unwrap();
        assert_eq!(r, json!({"behind": false, "rebuilt": false}), "{name}: opened, nothing to update");
        let r = s.execute("model.revise", &json!({"rebuild": false})).unwrap();
        assert_eq!(r["totals"]["moved"].as_u64().unwrap_or(0) + r["totals"]["added"].as_u64().unwrap_or(0), 0, "{name}: {r}");
        // Stretch the plan: the model is behind it and follows it on update.
        stretch_east(&mut s, 24.0);
        let r = s.execute("model.update", &json!({})).unwrap();
        assert_eq!(r, json!({"behind": true, "rebuilt": true}), "{name}: the reopened model follows its plan");
        assert!((east(&s) - (built_east + 24.0)).abs() < 1e-6, "{name}: walls moved with the plan: {} vs {}", east(&s), built_east + 24.0);
        // Nothing was edited by hand: the rebuild kept no edits and found no conflicts (the
        // saved fingerprints match the objects as opened).
        let last = s.execute("model.revise", &json!({"last": true})).unwrap();
        assert_eq!(last["totals"]["kept"], json!(0), "{name}: {last}");
        assert_eq!(last["totals"]["conflicts"], json!(0), "{name}: {last}");
        assert!(last["totals"]["moved"].as_u64().unwrap_or(0) + last["totals"]["added"].as_u64().unwrap_or(0) > 0, "{name}: {last}");
        // Saved again and reopened: the rebuilt model is the one recorded.
        s.execute("qsave", &json!({})).unwrap();
        drop(s);
        let mut s = reopen(&path);
        assert_eq!(s.execute("model.update", &json!({})).unwrap(), json!({"behind": false, "rebuilt": false}), "{name}");
        stretch_east(&mut s, -24.0);
        assert_eq!(s.execute("model.update", &json!({})).unwrap(), json!({"behind": true, "rebuilt": true}), "{name}");
        assert!((east(&s) - built_east).abs() < 1e-6, "{name}: back as built: {} vs {built_east}", east(&s));
    }
}

#[test]
fn a_model_that_does_not_follow_its_plan_stays_so_and_revise_works() {
    let _turn = serial();
    let d = scratch("reopen-manual");
    let mut s = plan(json!({"auto": false}));
    let built_east = east(&s);
    let path = d.join("m.cdy").to_string_lossy().into_owned();
    s.execute("saveas", &json!({ "path": path })).unwrap();
    drop(s);

    let mut s = reopen(&path);
    stretch_east(&mut s, 24.0);
    // auto off was saved: the plan moves alone.
    let r = s.execute("model.update", &json!({})).unwrap();
    assert_eq!(r, json!({"behind": false, "rebuilt": false}));
    assert!((east(&s) - built_east).abs() < 1e-6, "not rebuilt: auto is off");
    // model.revise compares with the plan the model was built from (saved with the drawing).
    let r = s.execute("model.revise", &json!({})).unwrap();
    let changed: u64 = ["added", "removed", "moved", "changed"].iter().filter_map(|k| r["totals"][k].as_u64()).sum();
    assert!(changed > 0, "the stretch is found: {r}");
    assert!((east(&s) - (built_east + 24.0)).abs() < 1e-6, "revised with the plan: {} vs {}", east(&s), built_east + 24.0);
}

fn walls(s: &Session) -> usize {
    s.doc().unwrap().model.iter().filter(|e| made_as(e).as_deref() == Some("wall")).count()
}

#[test]
fn an_edit_by_hand_survives_saving_and_the_next_rebuild() {
    let _turn = serial();
    let d = scratch("reopen-edit");
    let mut s = plan(json!({}));
    let n = walls(&s);
    // A built wall away from the east side, deleted by hand.
    let west: Vec<String> = s
        .doc()
        .unwrap()
        .model
        .iter()
        .filter(|e| made_as(e).as_deref() == Some("wall"))
        .filter(|e| matches!(&e.kind, EntityKind::Mesh(m) if m.vertices.iter().all(|v| v.x < 10.0)))
        .map(|e| e.handle.hex())
        .take(1)
        .collect();
    assert_eq!(west.len(), 1, "a west wall");
    s.execute("erase", &json!({"handles": west})).unwrap();
    assert_eq!(walls(&s), n - 1);
    let path = d.join("m.cdy").to_string_lossy().into_owned();
    s.execute("saveas", &json!({ "path": path })).unwrap();
    drop(s);

    let mut s = reopen(&path);
    stretch_east(&mut s, 24.0);
    assert_eq!(s.execute("model.update", &json!({})).unwrap(), json!({"behind": true, "rebuilt": true}));
    assert_eq!(walls(&s), n - 1, "the wall deleted by hand stays deleted after the rebuild");
    let last = s.execute("model.revise", &json!({"last": true})).unwrap();
    assert_eq!(last["deleted"], json!(1), "{last}");
}

#[test]
fn a_model_saved_before_its_state_was_kept_follows_its_plan() {
    let _turn = serial();
    let d = scratch("reopen-legacy");
    let mut s = plan(json!({}));
    let built_east = east(&s);
    let path = d.join("m.dxf").to_string_lossy().into_owned();
    s.execute("saveas", &json!({ "path": path })).unwrap();
    drop(s);
    // The same file as an older build saved it: no associative state in it.
    let mut old = cadaddy_io::read(&std::fs::read(&path).unwrap(), "m.dxf").unwrap();
    assert!(old.embedded.remove("model/associative.json").is_some(), "the state is saved with the drawing");
    let legacy = d.join("old.dxf").to_string_lossy().into_owned();
    std::fs::write(&legacy, cadaddy_io::write(&old, "old.dxf").unwrap()).unwrap();

    let mut s = reopen(&legacy);
    assert_eq!(s.execute("model.update", &json!({})).unwrap(), json!({"behind": false, "rebuilt": false}));
    stretch_east(&mut s, 24.0);
    assert_eq!(s.execute("model.update", &json!({})).unwrap(), json!({"behind": true, "rebuilt": true}));
    assert!((east(&s) - (built_east + 24.0)).abs() < 1e-6, "{} vs {}", east(&s), built_east + 24.0);
    // Its rooms weren't read on opening: the first revision takes them as they are, and reports
    // only the plan objects that changed.
    let last = s.execute("model.revise", &json!({"last": true})).unwrap();
    assert_eq!(last["totals"]["added"], json!(0), "{last}");
    assert_eq!(last["totals"]["removed"], json!(0), "{last}");
}
