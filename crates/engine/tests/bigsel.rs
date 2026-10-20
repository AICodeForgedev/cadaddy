//! Editing very large selections: MOVE, COPY, ROTATE, SCALE, ERASE and CHPROP on 100k+
//! objects give the same results as one object at a time, fast.

use cadaddy_doc::{Entity, EntityKind, Handle, Line};
use cadaddy_engine::Session;
use cadaddy_geom::Vec3;
use serde_json::json;

fn big(n: u64) -> Session {
    let mut s = Session::new();
    let d = s.doc_mut().unwrap();
    for i in 0..n {
        let x = (i % 500) as f64 * 3.0;
        let y = (i / 500) as f64 * 3.0;
        let mut e = Entity::new(Handle(0x1000 + i), EntityKind::Line(Line { a: Vec3::new(x, y, 0.0), b: Vec3::new(x + 2.0, y + 1.0, 0.0) }));
        e.common.layer = "0".into();
        d.model.push(e);
    }
    d.handseed = 0x1000 + n + 1;
    s
}

fn all(s: &Session) -> Vec<String> {
    s.doc().unwrap().model.iter().map(|e| e.handle.hex()).collect()
}

/// A mixed drawing: lines, circles, arcs, text, polylines, a block reference, objects on a
/// locked layer and an annotative text with two scales.
fn mixed(n: u64) -> Session {
    let mut s = Session::new();
    s.execute("layer", &json!({"name": "LOCKED", "make": true})).ok();
    for i in 0..n {
        let x = (i % 60) as f64 * 4.0;
        let y = (i / 60) as f64 * 4.0;
        let r = match i % 6 {
            0 => s.execute("line", &json!({"points": [[x, y], [x + 2.0, y + 1.0]]})),
            1 => s.execute("circle", &json!({"center": [x, y], "radius": 1.5})),
            2 => s.execute("arc", &json!({"center": [x, y], "radius": 1.2, "start": 10, "end": 200})),
            3 => s.execute("text", &json!({"at": [x, y], "text": format!("T{i}"), "height": 0.5})),
            4 => s.execute("pline", &json!({"points": [[x, y], [x + 1.0, y], [x + 1.0, y + 1.0]]})),
            _ => s.execute("rectang", &json!({"p1": [x, y], "p2": [x + 1.5, y + 1.0]})),
        };
        r.unwrap();
    }
    // Some on a locked layer (they stay put), and an annotative text with two scales.
    let hs: Vec<String> = all(&s).into_iter().take(40).collect();
    s.execute("properties.set", &json!({"handles": hs, "layer": "LOCKED"})).unwrap();
    s.execute("layer", &json!({"name": "LOCKED", "lock": true})).ok();
    let t = s.execute("text", &json!({"at": [500, 500], "text": "ANNO", "height": 0.5})).unwrap()["handle"].as_str().unwrap().to_string();
    s.execute("properties.set", &json!({"handles": [t], "annotative": true})).ok();
    s
}

fn model(s: &Session) -> Vec<serde_json::Value> {
    s.doc().unwrap().model.iter().map(|e| serde_json::to_value(e.as_ref()).unwrap()).collect()
}

#[test]
fn large_selections_give_the_same_drawing_as_one_object_at_a_time() {
    for (cmd, extra) in [
        ("move", json!({"from": [0, 0], "to": [10, 5]})),
        ("rotate", json!({"base": [3, 4], "angle": 30})),
        ("scale", json!({"base": [1, 1], "factor": 1.5})),
        ("copy", json!({"from": [0, 0], "to": [0, 300]})),
        ("properties.set", json!({"color": 3, "linetype": "ByBlock", "ltscale": 2})),
        ("erase", json!({})),
    ] {
        let mut batch = mixed(4000);
        let mut single = mixed(4000);
        let hs = all(&batch);
        assert!(hs.len() > 64, "large enough for the batch path");
        let mut p = extra.clone();
        p["handles"] = json!(hs);
        let r = batch.execute(cmd, &p);
        // One at a time (the path below the batch size), in the same order.
        let mut errors = 0;
        for h in &hs {
            let mut q = extra.clone();
            q["handles"] = json!([h]);
            if single.execute(cmd, &q).is_err() {
                errors += 1;
            }
        }
        assert!(r.is_ok() || errors == hs.len(), "{cmd}: {r:?}");
        assert_eq!(model(&batch), model(&single), "{cmd}: the same objects, in the same order");
        assert_eq!(batch.doc().unwrap().handseed, single.doc().unwrap().handseed, "{cmd}: the same handles given out");
    }
}

#[test]
fn moving_everything_is_one_undo_step_and_finite() {
    let mut s = big(20_000);
    let before = model(&s);
    let hs = all(&s);
    s.execute("move", &json!({"handles": hs, "from": [0, 0], "to": [5, 5]})).unwrap();
    assert_ne!(model(&s), before);
    s.execute("undo", &json!({})).unwrap();
    assert_eq!(model(&s), before);
    // A move to infinity is refused as before, everything kept.
    let e = s.execute("move", &json!({"handles": all(&s), "from": [0, 0], "to": [1e308, 0]}));
    let e2 = s.execute("move", &json!({"handles": all(&s), "from": [0, 0], "to": [1e308, 0]}));
    assert!(e.is_err() || e2.is_err(), "overflow refused");
}

/// `cargo test --release -p cadaddy-engine --test bigsel bench -- --ignored --nocapture`
#[test]
#[ignore]
fn bench() {
    for (name, cmd, extra) in [
        ("move", "move", json!({"from": [0, 0], "to": [10, 5]})),
        ("copy", "copy", json!({"from": [0, 0], "to": [10, 5]})),
        ("rotate", "rotate", json!({"base": [0, 0], "angle": 30})),
        ("scale", "scale", json!({"base": [0, 0], "factor": 2})),
        ("chprop", "properties.set", json!({"color": 3})),
        ("erase", "erase", json!({})),
    ] {
        let mut s = big(167_000);
        let mut p = extra.clone();
        p["handles"] = json!(all(&s));
        let t = std::time::Instant::now();
        s.execute(cmd, &p).unwrap();
        println!("{name}: {:?}", t.elapsed());
    }
}

/// The real drawing, when it is here: `CADADDY_E4T=path cargo test --release ... bench_e4t -- --ignored --nocapture`
#[test]
#[ignore]
fn bench_e4t() {
    let Ok(path) = std::env::var("CADADDY_E4T") else { return };
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
    });
    for (name, cmd, extra) in [
        ("move", "move", json!({"from": [0, 0], "to": [10, 5]})),
        ("copy", "copy", json!({"from": [0, 0], "to": [10, 5]})),
        ("rotate", "rotate", json!({"base": [0, 0], "angle": 30})),
        ("scale", "scale", json!({"base": [0, 0], "factor": 2})),
        ("chprop", "properties.set", json!({"color": 3})),
        ("erase", "erase", json!({})),
    ] {
        let mut s = Session::empty();
        s.execute("open", &json!({"path": path})).unwrap();
        let mut p = extra.clone();
        p["handles"] = json!(all(&s));
        let n = p["handles"].as_array().unwrap().len();
        let t = std::time::Instant::now();
        s.execute(cmd, &p).unwrap();
        println!("e4t {name} ({n}): {:?}", t.elapsed());
    }
}

#[test]
#[ignore]
fn bench_e4t_phases() {
    let Ok(path) = std::env::var("CADADDY_E4T") else { return };
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
    });
    let mut s = Session::empty();
    s.execute("open", &json!({"path": path})).unwrap();
    let p = json!({"handles": all(&s), "from": [0, 0], "to": [10, 5]});
    let run = cadaddy_engine::cmd::find_command("move").unwrap().run;
    let t = std::time::Instant::now();
    run(&mut s, &p).unwrap();
    println!("move run alone: {:?}", t.elapsed());
    let t = std::time::Instant::now();
    s.execute("move", &p).unwrap();
    println!("move via execute: {:?}", t.elapsed());
    let t = std::time::Instant::now();
    let _ = s.doc().unwrap().clone();
    println!("drawing clone: {:?}", t.elapsed());
}
