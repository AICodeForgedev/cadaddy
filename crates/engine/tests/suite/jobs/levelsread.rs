//! Levels come from the floor plans, never a site spot elevation (Q5277): four floor plans side
//! by side build four stacked storeys; reading the prints' level notes afterwards (a civil
//! sheet's "FIRST FLOOR FFE 147'-0"" among them) keeps the levels in order, and building again
//! still stacks every floor on its own level.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::wallgen::made_as;
use cadaddy_engine::doc::{Entity, EntityKind, entity_bounds};
use cadaddy_engine::geom::Bounds2;
use serde_json::json;

/// The plans' spacing on the sheet (inches).
const STEP: f64 = 1200.0;
const PLANS: usize = 4;

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("line", &json!({"points": [a, b]})).unwrap();
}

fn rect(s: &mut Session, x0: f64, y0: f64, x1: f64, y1: f64) {
    line(s, [x0, y0], [x1, y0]);
    line(s, [x1, y0], [x1, y1]);
    line(s, [x1, y1], [x0, y1]);
    line(s, [x0, y1], [x0, y0]);
}

/// Four 40' × 30' floors side by side (6" outside walls, a partition), then a site plan and a
/// section sheet beyond them.
fn prints() -> Session {
    let mut s = Session::new();
    s.execute("layer.new", &json!({"name": "A-WALL", "current": true})).unwrap();
    for i in 0..PLANS {
        let ox = i as f64 * STEP;
        rect(&mut s, ox, 0.0, ox + 480.0, 360.0);
        rect(&mut s, ox + 6.0, 6.0, ox + 474.0, 354.0);
        line(&mut s, [ox + 238.0, 6.0], [ox + 238.0, 354.0]);
        line(&mut s, [ox + 242.0, 6.0], [ox + 242.0, 354.0]);
    }
    s.execute("layer.new", &json!({"name": "C-ANNO", "current": true})).unwrap();
    let site = PLANS as f64 * STEP;
    s.execute("text", &json!({"at": [site + 100.0, 200.0], "text": "FIRST FLOOR FFE 147'-0\"", "height": 8.0})).unwrap();
    s.execute("text", &json!({"at": [site + 100.0, -100.0], "text": "GRADING AND DRAINAGE PLAN", "height": 12.0})).unwrap();
    s
}

fn region(i: usize) -> [f64; 4] {
    let ox = i as f64 * STEP;
    [ox - 200.0, -200.0, ox + 800.0, 600.0]
}

fn level_of(e: &Entity) -> Option<String> {
    made_as(e)?;
    e.common.xdata.iter().filter(|t| t.code == 1000).nth(1).map(|t| t.value.clone())
}

/// Each level's walls: (level, plan extents, lowest z, highest z).
fn storeys(s: &Session) -> Vec<(String, Bounds2, f64, f64)> {
    let d = s.doc().unwrap();
    let mut out: Vec<(String, Bounds2, f64, f64)> = Vec::new();
    for e in d.model.iter().filter(|e| made_as(e).as_deref() == Some("wall")) {
        let Some(l) = level_of(e) else { continue };
        let i = match out.iter().position(|x| x.0 == l) {
            Some(i) => i,
            None => {
                out.push((l, Bounds2::EMPTY, f64::INFINITY, f64::NEG_INFINITY));
                out.len() - 1
            }
        };
        out[i].1 = out[i].1.union(&entity_bounds(d, e, 0));
        if let EntityKind::Mesh(m) = &e.kind {
            for v in &m.vertices {
                out[i].2 = out[i].2.min(v.z);
                out[i].3 = out[i].3.max(v.z);
            }
        }
    }
    out.sort_by(|a, b| a.2.total_cmp(&b.2));
    out
}

fn assert_stacked(s: &Session, what: &str) {
    let st = storeys(s);
    let names: Vec<&str> = st.iter().map(|x| x.0.as_str()).collect();
    assert_eq!(names, ["Level 1", "Level 2", "Level 3", "Level 4"], "{what}: built lowest first");
    let first = st[0].1;
    for w in st.windows(2) {
        assert!(w[1].2 >= w[0].3 - 1.0, "{what}: {} sits on {}: {:?}", w[1].0, w[0].0, (w[0].2, w[0].3, w[1].2, w[1].3));
    }
    for x in &st {
        // Same x/y as the first floor: one stacked building, never side by side.
        assert!(x.1.min.dist(first.min) < 1e-6 && x.1.max.dist(first.max) < 1e-6, "{what}: {} at {:?} vs {:?}", x.0, x.1, first);
    }
}

#[test]
fn every_floor_plan_builds_on_its_own_level_and_a_site_elevation_never_moves_one() {
    let mut s = prints();
    let levels: Vec<_> = (0..PLANS)
        .map(|i| json!({"name": format!("Level {}", i + 1), "elevation_ft": 12.0 * i as f64, "height_ft": 12.0, "ceiling_ft": 9.0, "region": region(i)}))
        .collect();
    s.execute("bspec.draft", &json!({"store": true, "levels": levels, "stack": true})).unwrap();
    let r = s.execute("model.build", &json!({})).unwrap();
    assert_eq!(r["levels"], json!(PLANS), "{r}");
    assert_eq!(r["stacked"], json!(PLANS - 1), "{r}");
    assert_stacked(&s, "first build");

    // Read the prints' level notes and apply them (the blind test's step): the civil sheet's
    // first floor at 147'-0" would put Level 1 over Levels 3 and 4.
    let read = s.execute("levels.read", &json!({"apply": true})).unwrap();
    assert_eq!(read["warnings"].as_array().map(Vec::len), Some(1), "{read}");
    let list = s.execute("levels.list", &json!({})).unwrap();
    let el: Vec<f64> = list["levels"].as_array().unwrap().iter().filter_map(|l| l["elevation_ft"].as_f64()).collect();
    assert_eq!(el, [0.0, 12.0, 24.0, 36.0], "{list}");
    let built: Vec<u64> = list["levels"].as_array().unwrap().iter().filter_map(|l| l["built"].as_u64()).collect();
    assert!(built.iter().all(|n| *n > 0), "every level has its built objects: {list}");

    // Building again keeps every floor on its level, stacked.
    let r = s.execute("model.build", &json!({})).unwrap();
    assert_eq!(r["levels"], json!(PLANS), "{r}");
    assert_stacked(&s, "rebuild");
}
