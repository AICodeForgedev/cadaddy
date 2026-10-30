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

/// Objects of `mixed()` put on the locked layer.
const LOCKED_N: usize = 40;
/// Index (in drawing order) of a line that stays unlocked: i = 42, so x = 168, y = 0.
const FREE_LINE: usize = 42;

/// A mixed drawing: lines, circles, arcs, text, polylines and rectangles on layer "0", the first
/// 40 of them moved onto a layer that is then locked, and an annotative text with two scales.
fn mixed(n: u64) -> Session {
    let mut s = Session::new();
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
    let t = s.execute("text", &json!({"at": [500, 500], "text": "ANNO", "height": 0.5})).unwrap()["handle"].as_str().unwrap().to_string();
    s.execute("properties.set", &json!({"handles": [t], "annotative": true})).unwrap();
    s.execute("objectscale", &json!({"handles": [t], "add": "1:50"})).unwrap();
    let anno = s.doc().unwrap().model.iter().find(|e| e.handle.hex() == t).unwrap().common.anno_scales.len();
    assert_eq!(anno, 2, "the annotative text has two scales, so the multi-scale path runs");
    // A locked layer (not the current one) holding 40 objects that must stay put.
    s.execute("layer.new", &json!({"name": "LOCKED"})).unwrap();
    let hs: Vec<String> = all(&s).into_iter().take(LOCKED_N).collect();
    s.execute("properties.set", &json!({"handles": hs, "layer": "LOCKED"})).unwrap();
    s.execute("layer.set", &json!({"name": "LOCKED", "locked": true})).unwrap();
    let d = s.doc().unwrap();
    assert_eq!(d.model.iter().filter(|e| e.common.layer == "LOCKED").count(), LOCKED_N);
    assert_eq!(d.model.iter().filter(|e| e.common.layer == "0").count(), n as usize + 1 - LOCKED_N);
    s
}

fn model(s: &Session) -> Vec<serde_json::Value> {
    s.doc().unwrap().model.iter().map(|e| serde_json::to_value(e.as_ref()).unwrap()).collect()
}

/// The ends of the line at drawing-order index `i`.
fn line_at(s: &Session, i: usize) -> (Vec3, Vec3) {
    match &s.doc().unwrap().model.iter().nth(i).unwrap().kind {
        EntityKind::Line(l) => (l.a, l.b),
        k => panic!("not a line: {k:?}"),
    }
}

fn near(a: Vec3, x: f64, y: f64) -> bool {
    (a.x - x).abs() < 1e-9 && (a.y - y).abs() < 1e-9
}

#[test]
fn large_selections_give_the_same_drawing_as_one_object_at_a_time() {
    let total = 4001usize;
    let (c, sn) = (30f64.to_radians().cos(), 30f64.to_radians().sin());
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
        assert_eq!(hs.len(), total);
        assert!(hs.len() > 64, "large enough for the batch path");
        let (free0, locked0) = (line_at(&batch, FREE_LINE), line_at(&batch, 0));
        let mut p = extra.clone();
        p["handles"] = json!(hs);
        let r = batch.execute(cmd, &p);
        // One at a time (the path below the batch size), in the same order.
        for h in &hs {
            let mut q = extra.clone();
            q["handles"] = json!([h]);
            let _ = single.execute(cmd, &q);
        }
        let r = r.unwrap_or_else(|e| panic!("{cmd}: the batch call failed: {e:?}"));
        // The batch result is checked against known values, not only against the twin.
        let (free, locked) = if cmd == "erase" { (free0, locked0) } else { (line_at(&batch, FREE_LINE), line_at(&batch, 0)) };
        match cmd {
            "move" => {
                assert!(near(free.0, free0.0.x + 10.0, free0.0.y + 5.0) && near(free.1, free0.1.x + 10.0, free0.1.y + 5.0), "{cmd}: free line");
                assert_eq!((locked.0, locked.1), (locked0.0, locked0.1), "{cmd}: the locked line did not move");
            }
            "rotate" => {
                let rot = |v: Vec3| (3.0 + (v.x - 3.0) * c - (v.y - 4.0) * sn, 4.0 + (v.x - 3.0) * sn + (v.y - 4.0) * c);
                let (ex, ey) = rot(free0.0);
                assert!(near(free.0, ex, ey), "{cmd}: free line start {:?} vs {ex},{ey}", free.0);
                assert_eq!(locked.0, locked0.0, "{cmd}: the locked line did not turn");
            }
            "scale" => {
                let sc = |v: Vec3| (1.0 + (v.x - 1.0) * 1.5, 1.0 + (v.y - 1.0) * 1.5);
                let (ex, ey) = sc(free0.1);
                assert!(near(free.1, ex, ey), "{cmd}: free line end {:?} vs {ex},{ey}", free.1);
                assert_eq!(locked.1, locked0.1, "{cmd}: the locked line did not scale");
            }
            "copy" => assert_eq!(batch.doc().unwrap().model.iter().count(), 2 * total, "{cmd}: every object copied once"),
            "properties.set" => assert_eq!(r["changed"], total - LOCKED_N, "{cmd}: the unlocked objects changed"),
            "erase" => {
                let d = batch.doc().unwrap();
                assert_eq!(d.model.iter().count(), LOCKED_N, "{cmd}: only the locked objects are left");
                assert!(d.model.iter().all(|e| e.common.layer == "LOCKED"));
            }
            _ => unreachable!(),
        }
        assert_eq!(model(&batch), model(&single), "{cmd}: the same objects, in the same order");
        assert_eq!(batch.doc().unwrap().handseed, single.doc().unwrap().handseed, "{cmd}: the same handles given out");
    }
}

#[test]
fn moving_everything_is_one_undo_step_and_finite() {
    let mut s = big(20_000);
    let before = model(&s);
    let (a0, b0) = line_at(&s, 7);
    let hs = all(&s);
    s.execute("move", &json!({"handles": hs, "from": [0, 0], "to": [5, 5]})).unwrap();
    let (a1, b1) = line_at(&s, 7);
    assert!(near(a1, a0.x + 5.0, a0.y + 5.0) && near(b1, b0.x + 5.0, b0.y + 5.0), "the line moved by (5,5)");
    s.execute("undo", &json!({})).unwrap();
    assert_eq!(model(&s), before);
    assert_eq!(line_at(&s, 7), (a0, b0), "one undo put the line back");
    // A move to infinity is refused, and everything is kept.
    assert!(s.execute("move", &json!({"handles": all(&s), "from": [0, 0], "to": [1e308, 0]})).is_err(), "overflow refused");
    assert_eq!(model(&s), before, "the refused move changed nothing");
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
        read_dwf: None,
        write_dwfx: None,
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
        read_dwf: None,
        write_dwfx: None,
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
