//! Structural grids: two floors drawn at different origins of the sheet align exactly from their
//! grid intersections (a third drawn at half size is reported, not moved), and the columns drawn
//! at the intersections become columns through the storey.

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

fn text(s: &mut Session, at: [f64; 2], t: &str) {
    s.execute("text", &json!({"at": at, "text": t, "height": 8})).unwrap();
}

/// A floor at `o`, scaled `k`: grids A and B (x 0 and 300) bubbled below, 1 and 2 (y 0 and
/// 240) bubbled at the left; a wall line along grid 1; 12" columns at the four intersections.
fn floor(s: &mut Session, o: [f64; 2], k: f64) {
    let p = |x: f64, y: f64| [o[0] + x * k, o[1] + y * k];
    layer(s, "S-GRID");
    for (x, label) in [(0.0, "A"), (300.0, "B")] {
        line(s, p(x, -96.0 + 18.0), p(x, 300.0));
        s.execute("circle", &json!({"center": p(x, -96.0), "radius": 18.0 * k})).unwrap();
        text(s, p(x - 4.0, -100.0), label);
    }
    for (y, label) in [(0.0, "1"), (240.0, "2")] {
        line(s, p(-96.0 + 18.0, y), p(360.0, y));
        s.execute("circle", &json!({"center": p(-96.0, y), "radius": 18.0 * k})).unwrap();
        text(s, p(-100.0, y - 4.0), label);
    }
    layer(s, "S-COLS");
    for (x, y) in [(0.0, 0.0), (300.0, 0.0), (0.0, 240.0), (300.0, 240.0)] {
        let (a, b) = (p(x - 6.0, y - 6.0), p(x + 6.0, y + 6.0));
        s.execute("pline", &json!({"vertices": [a, [b[0], a[1]], b, [a[0], b[1]]], "closed": true})).unwrap();
    }
    layer(s, "A-WALL");
    line(s, p(0.0, 0.0), p(300.0, 0.0));
}

fn drawing() -> Session {
    let mut s = Session::new();
    floor(&mut s, [0.0, 0.0], 1.0);
    floor(&mut s, [2100.0, 50.0], 1.0);
    floor(&mut s, [5000.0, 0.0], 0.5);
    s.execute(
        "bspec.set",
        &json!({"replace": true, "spec": {
            "levels": [
                {"name": "L1", "elevation_ft": 0, "height_ft": 12, "region": [-200, -200, 1000, 1000]},
                {"name": "L2", "elevation_ft": 12, "height_ft": 12, "region": [1800, -200, 3000, 1000]},
                {"name": "L3", "elevation_ft": 24, "height_ft": 12, "region": [4800, -200, 6000, 1000]}
            ],
            "layers": [{"layer": "A-WALL", "role": "wall"}]
        }}),
    )
    .unwrap();
    s
}

fn level<'a>(r: &'a Value, name: &str) -> &'a Value {
    r["levels"].as_array().unwrap().iter().find(|l| l["level"] == name).unwrap_or_else(|| panic!("{name}: {r}"))
}

#[test]
fn grids_are_found_with_their_intersections() {
    let mut s = drawing();
    let r = s.execute("grids.detect", &json!({"level": "L2"})).unwrap();
    let l2 = level(&r, "L2");
    let labels: Vec<&str> = l2["grids"].as_array().unwrap().iter().map(|g| g["label"].as_str().unwrap()).collect();
    assert_eq!(labels.len(), 4, "{r}");
    let xs: Vec<(String, Value)> =
        l2["intersections"].as_array().unwrap().iter().map(|x| (x["label"].as_str().unwrap().to_string(), x["at"].clone())).collect();
    assert_eq!(
        xs,
        vec![
            ("A-1".to_string(), json!([2100.0, 50.0])),
            ("A-2".to_string(), json!([2100.0, 290.0])),
            ("B-1".to_string(), json!([2400.0, 50.0])),
            ("B-2".to_string(), json!([2400.0, 290.0]))
        ]
    );
    assert!(s.execute("grids.detect", &json!({"level": "L9"})).is_err());
}

#[test]
fn two_floors_at_different_origins_align_exactly() {
    let mut s = drawing();
    let r = s.execute("levels.align", &json!({})).unwrap();
    assert_eq!(r["reference"], json!("L1"));
    let l2 = level(&r, "L2");
    assert_eq!(
        (l2["matched"].clone(), l2["scale"].clone(), l2["offset"].clone(), l2["residual"].clone()),
        (json!(4), json!(1.0), json!([-2100.0, -50.0]), json!(0.0))
    );
    assert_eq!(l2["base"], json!([2100.0, 50.0]));
    // The half-size floor is found at its scale and left alone.
    let l3 = level(&r, "L3");
    assert_eq!(l3["scale"], json!(2.0));
    assert!(l3["note"].as_str().unwrap().contains("scale"), "{l3}");
    assert!(l3.get("base").is_none());
    s.execute("levels.align", &json!({"apply": true})).unwrap();
    let sp = s.execute("bspec.get", &json!({})).unwrap()["spec"].clone();
    assert_eq!((sp["levels"][0]["base"].clone(), sp["levels"][1]["base"].clone()), (json!([0.0, 0.0]), json!([2100.0, 50.0])));
    assert!(sp["levels"][2]["base"].is_null());
    // Stacked: Walk mode's wall along grid 1 is in the same place on both floors.
    let spec = cadaddy_engine::walk3d::BuildingSpec::from_json(&sp.to_string()).unwrap();
    let scene = cadaddy_engine::walk3d::derive(s.doc().unwrap(), &spec.to_rules());
    let wall = |lv: usize| scene.walls.iter().find(|w| w.level == lv).map(|w| (w.a.x, w.a.y, w.b.x, w.b.y)).unwrap();
    assert_eq!(wall(0), wall(1));
    assert!(s.execute("levels.align", &json!({"reference": "L9"})).is_err());
}

#[test]
fn columns_at_the_intersections() {
    let mut s = drawing();
    let r = s.execute("grids.columns", &json!({"level": "L1", "to_level": "L2", "apply": true})).unwrap();
    let cols = r["columns"].as_array().unwrap();
    assert_eq!(cols.len(), 4, "{r}");
    assert!(cols.iter().all(|c| c["size_in"] == json!([12.0, 12.0]) && c["round"] == json!(false)));
    assert_eq!(cols[0]["grid"], json!("A-1"));
    let b = s.execute("model.build", &json!({})).unwrap();
    assert_eq!(b["gridColumns"], json!(4), "{b}");
    let heights: Vec<(f64, f64)> = s
        .doc()
        .unwrap()
        .model
        .iter()
        .filter(|e| made_as(e).as_deref() == Some("column"))
        .filter_map(|e| {
            if let EntityKind::Mesh(m) = &e.kind {
                Some(m.vertices.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(v.z), hi.max(v.z))))
            } else {
                None
            }
        })
        .collect();
    assert_eq!(heights, vec![(0.0, 288.0); 4], "through L1 and L2");
    assert!(s.execute("grids.columns", &json!({"level": "L1", "to_level": "L9"})).is_err());
}

fn circle(s: &mut Session, at: [f64; 2], r: f64) {
    s.execute("circle", &json!({"center": at, "radius": r})).unwrap();
}

/// The plain floor's grids, columns and wall at `o`, its grids labelled `labels` (the grids at
/// x 0 and 300, then y 0 and 240) and, with `far`, a second bubble at the far end of each line.
fn grid_floor(s: &mut Session, o: [f64; 2], labels: [&str; 4], far: bool) {
    let p = |x: f64, y: f64| [o[0] + x, o[1] + y];
    layer(s, "S-GRID");
    for (x, label) in [(0.0, labels[0]), (300.0, labels[1])] {
        line(s, p(x, -78.0), p(x, 300.0));
        circle(s, p(x, -96.0), 18.0);
        text(s, p(x - 4.0, -100.0), label);
        if far {
            circle(s, p(x, 318.0), 18.0);
            text(s, p(x - 4.0, 314.0), label);
        }
    }
    for (y, label) in [(0.0, labels[2]), (240.0, labels[3])] {
        line(s, p(-78.0, y), p(360.0, y));
        circle(s, p(-96.0, y), 18.0);
        text(s, p(-100.0, y - 4.0), label);
        if far {
            circle(s, p(378.0, y), 18.0);
            text(s, p(374.0, y - 4.0), label);
        }
    }
    layer(s, "S-COLS");
    for (x, y) in [(0.0, 0.0), (300.0, 0.0), (0.0, 240.0), (300.0, 240.0)] {
        let (a, b) = (p(x - 6.0, y - 6.0), p(x + 6.0, y + 6.0));
        s.execute("pline", &json!({"vertices": [a, [b[0], a[1]], b, [a[0], b[1]]], "closed": true})).unwrap();
    }
    layer(s, "A-WALL");
    line(s, p(0.0, 0.0), p(300.0, 0.0));
}

/// Two levels side by side: `L1` drawn by `first` at the origin, `L2` by `second` at (2100, 50).
fn two_levels(first: impl Fn(&mut Session, [f64; 2]), second: impl Fn(&mut Session, [f64; 2])) -> Session {
    let mut s = Session::new();
    first(&mut s, [0.0, 0.0]);
    second(&mut s, [2100.0, 50.0]);
    s.execute(
        "bspec.set",
        &json!({"replace": true, "spec": {
            "levels": [
                {"name": "L1", "elevation_ft": 0, "height_ft": 12, "region": [-200, -200, 1000, 1000]},
                {"name": "L2", "elevation_ft": 12, "height_ft": 12, "region": [1800, -200, 3000, 1000]}
            ],
            "layers": [{"layer": "A-WALL", "role": "wall"}]
        }}),
    )
    .unwrap();
    s
}

fn labels(l: &Value) -> Vec<String> {
    let mut v: Vec<String> = l["grids"].as_array().unwrap().iter().map(|g| g["label"].as_str().unwrap().to_string()).collect();
    v.sort();
    v
}

#[test]
fn bubbles_at_both_ends_are_one_grid() {
    let mut s = two_levels(|s, o| grid_floor(s, o, ["A", "B", "1", "2"], true), |s, o| grid_floor(s, o, ["A", "B", "1", "2"], true));
    let r = s.execute("grids.detect", &json!({})).unwrap();
    for (name, o) in [("L1", [0.0, 0.0]), ("L2", [2100.0, 50.0])] {
        let l = level(&r, name);
        assert_eq!(labels(l), vec!["1", "2", "A", "B"], "{l}");
        // Each grid runs the whole line, from its first bubble's end.
        let a = l["grids"].as_array().unwrap().iter().find(|g| g["label"] == "A").unwrap();
        assert_eq!((a["from"].clone(), a["to"].clone()), (json!([o[0], o[1] - 78.0]), json!([o[0], o[1] + 300.0])), "{l}");
        let xs: Vec<(String, Value)> =
            l["intersections"].as_array().unwrap().iter().map(|x| (x["label"].as_str().unwrap().to_string(), x["at"].clone())).collect();
        assert_eq!(
            xs,
            vec![
                ("A-1".to_string(), json!([o[0], o[1]])),
                ("A-2".to_string(), json!([o[0], o[1] + 240.0])),
                ("B-1".to_string(), json!([o[0] + 300.0, o[1]])),
                ("B-2".to_string(), json!([o[0] + 300.0, o[1] + 240.0]))
            ],
            "{l}"
        );
        let cols = s.execute("grids.columns", &json!({"level": name})).unwrap();
        let grids: Vec<&str> = cols["columns"].as_array().unwrap().iter().map(|c| c["grid"].as_str().unwrap()).collect();
        assert_eq!(grids, vec!["A-1", "A-2", "B-1", "B-2"], "{cols}");
    }
    s.execute("grids.columns", &json!({"level": "L1", "apply": true})).unwrap();
    let b = s.execute("model.build", &json!({})).unwrap();
    assert_eq!(b["gridColumns"], json!(4), "{b}");
}

/// The plain floor drawn the way exported plans often are: bubbles as the parts library's
/// G_GRID_BUBBLE_050 (lettered grids) and G_GRID_BUBBLE_HEX (numbered) blocks labelled by their
/// attribute, grid lines as polylines of two collinear segments.
fn block_floor(s: &mut Session, o: [f64; 2]) {
    let p = |x: f64, y: f64| [o[0] + x, o[1] + y];
    layer(s, "S-GRID");
    let bubble = |s: &mut Session, part: &str, at: [f64; 2], label: &str| {
        // 72× the part's ½" bubble: 18" across the radius, as the plain floor's circles.
        s.execute("part", &json!({"name": part, "at": at, "scale": 72.0, "attribs": {"GRID": label}, "layer": "current"})).unwrap();
    };
    for (x, label) in [(0.0, "A"), (300.0, "B")] {
        s.execute("pline", &json!({"vertices": [p(x, -78.0), p(x, 100.0), p(x, 300.0)]})).unwrap();
        bubble(s, "G_GRID_BUBBLE_050", p(x, -96.0), label);
    }
    for (y, label) in [(0.0, "1"), (240.0, "2")] {
        s.execute("pline", &json!({"vertices": [p(-78.0, y), p(150.0, y), p(360.0, y)]})).unwrap();
        bubble(s, "G_GRID_BUBBLE_HEX", p(-96.0, y), label);
    }
    layer(s, "S-COLS");
    for (x, y) in [(0.0, 0.0), (300.0, 0.0), (0.0, 240.0), (300.0, 240.0)] {
        let (a, b) = (p(x - 6.0, y - 6.0), p(x + 6.0, y + 6.0));
        s.execute("pline", &json!({"vertices": [a, [b[0], a[1]], b, [a[0], b[1]]], "closed": true})).unwrap();
    }
}

#[test]
fn block_bubbles_and_polyline_grids_read_as_the_plain_ones() {
    let mut plain = drawing();
    let mut blocks = two_levels(block_floor, block_floor);
    let inserts = blocks.doc().unwrap().model.iter().filter(|e| matches!(e.kind, EntityKind::Insert(_))).count();
    assert_eq!(inserts, 8, "two floors of four bubble blocks");
    let want = plain.execute("grids.detect", &json!({"level": "L1"})).unwrap();
    let got = blocks.execute("grids.detect", &json!({"level": "L1"})).unwrap();
    let (want, got) = (level(&want, "L1"), level(&got, "L1"));
    assert_eq!(got["intersections"], want["intersections"], "{got}");
    let by_label = |l: &Value| {
        let mut v: Vec<Value> = l["grids"].as_array().unwrap().clone();
        v.sort_by_key(|g| g["label"].as_str().unwrap().to_string());
        v
    };
    assert_eq!(by_label(got), by_label(want), "{got}");
    assert_eq!(
        by_label(got)[2],
        json!({"label": "A", "from": [0.0, -78.0], "to": [0.0, 300.0], "bubble": [0.0, -96.0]}),
        "the block's centre is the bubble"
    );
    // The second floor's too, and both line up by them.
    let r = blocks.execute("levels.align", &json!({})).unwrap();
    let l2 = level(&r, "L2");
    assert_eq!(
        (l2["matched"].clone(), l2["offset"].clone(), l2["residual"].clone(), l2["base"].clone()),
        (json!(4), json!([-2100.0, -50.0]), json!(0.0), json!([2100.0, 50.0]))
    );
}

/// The handle of the closed outline centred on `at`.
fn outline_at(s: &Session, at: [f64; 2]) -> String {
    let d = s.doc().unwrap();
    d.model
        .iter()
        .find(|e| {
            let EntityKind::LwPolyline(pl) = &e.kind else { return false };
            let n = pl.vertices.len() as f64;
            let c = pl.vertices.iter().fold((0.0, 0.0), |a, v| (a.0 + v.p.x / n, a.1 + v.p.y / n));
            pl.closed && (c.0 - at[0]).abs() < 1e-9 && (c.1 - at[1]).abs() < 1e-9
        })
        .map(|e| e.handle.hex())
        .unwrap_or_else(|| panic!("no outline at {at:?}"))
}

#[test]
fn built_columns_name_their_drawn_outlines() {
    let mut s = drawing();
    let r = s.execute("grids.columns", &json!({"level": "L1", "apply": true})).unwrap();
    let want: Vec<(String, String)> = [("A-1", [0.0, 0.0]), ("A-2", [0.0, 240.0]), ("B-1", [300.0, 0.0]), ("B-2", [300.0, 240.0])]
        .iter()
        .map(|(g, at)| (g.to_string(), outline_at(&s, *at)))
        .collect();
    let cols: Vec<(String, String, String)> = r["columns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (c["grid"].as_str().unwrap().to_string(), c["source"].as_str().unwrap().to_string(), c["status"].as_str().unwrap().to_string()))
        .collect();
    assert_eq!(cols, want.iter().map(|(g, h)| (g.clone(), h.clone(), "rule".to_string())).collect::<Vec<_>>());
    // Kept in the spec.
    let sp = s.execute("bspec.get", &json!({})).unwrap()["spec"].clone();
    assert_eq!((sp["columns"][0]["source"].clone(), sp["columns"][0]["status"].clone()), (json!(want[0].1), json!("rule")));
    let b = s.execute("model.build", &json!({})).unwrap();
    assert_eq!(b["gridColumns"], json!(4), "{b}");
    // Each built column's build tags: [column, level, source handle, grid:…, status:rule].
    let mut built: Vec<(String, String, String)> = s
        .doc()
        .unwrap()
        .model
        .iter()
        .filter(|e| made_as(e).as_deref() == Some("column"))
        .map(|e| {
            let x = &e.common.xdata;
            let at = x.iter().position(|t| t.code == 1000 && t.value == "column").unwrap();
            let v = |i: usize| x.get(at + i).map(|t| t.value.clone()).unwrap_or_default();
            (v(3).trim_start_matches("grid:").to_string(), v(2), v(4))
        })
        .collect();
    built.sort();
    assert_eq!(built, want.iter().map(|(g, h)| (g.clone(), h.clone(), "status:rule".to_string())).collect::<Vec<_>>());
}

#[test]
fn a_fit_that_does_not_hold_is_not_applied() {
    // L2's grid B is labelled A (two grids named A), L3's grids share only A-2 with L1's.
    let mut s = two_levels(|s, o| grid_floor(s, o, ["A", "B", "1", "2"], false), |s, o| grid_floor(s, o, ["A", "A", "1", "2"], false));
    grid_floor(&mut s, [4000.0, 0.0], ["A", "C", "3", "2"], false);
    let mut sp = s.execute("bspec.get", &json!({})).unwrap()["spec"].clone();
    sp["levels"].as_array_mut().unwrap().push(json!({"name": "L3", "elevation_ft": 24, "height_ft": 12, "region": [3800, -200, 5000, 1000]}));
    s.execute("bspec.set", &json!({"replace": true, "spec": sp})).unwrap();
    let r = s.execute("levels.align", &json!({"apply": true})).unwrap();
    assert_eq!(r["reference"], json!("L1"));
    let l2 = level(&r, "L2");
    assert_eq!(l2["matched"], json!(4), "{l2}");
    assert!(l2["residual"].as_f64().unwrap() > 1.0, "{l2}");
    assert!(l2.get("base").is_none(), "{l2}");
    assert!(l2["note"].as_str().unwrap().starts_with("its grids don't line up with L1's: an intersection misses by"), "{l2}");
    assert!(l2["note"].as_str().unwrap().contains("check its grid labels with grids.detect"), "{l2}");
    let l3 = level(&r, "L3");
    assert_eq!((l3["matched"].clone(), l3.get("base")), (json!(1), None), "{l3}");
    assert_eq!(
        l3["note"],
        json!(
            "only grid intersection A-2 is in common with L1: not aligned (two or more are needed to check the fit); check its grid labels with grids.detect, or set its base by hand"
        )
    );
    // Applied: L1 has its base, L2 and L3 are left as they were.
    let sp = s.execute("bspec.get", &json!({})).unwrap()["spec"].clone();
    assert_eq!(
        (sp["levels"][0]["base"].clone(), sp["levels"][1]["base"].clone(), sp["levels"][2]["base"].clone()),
        (json!([0.0, 0.0]), Value::Null, Value::Null)
    );
}

fn io_hooks() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
        read_dwf: None,
        write_dwfx: None,
    });
}

#[test]
fn a_level_on_its_own_sheet_is_aligned_from_its_grids() {
    io_hooks();
    // Level 2's plan is its own drawing, drawn at (700, 300) of that sheet.
    let mut other = Session::new();
    floor(&mut other, [700.0, 300.0], 1.0);
    let dir = std::env::temp_dir().join(format!("cadaddy-grids-sheet-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("Sample Level 2.dxf");
    std::fs::write(&file, cadaddy_io::write(other.doc().unwrap(), "level2.dxf").unwrap()).unwrap();
    let mut s = Session::new();
    floor(&mut s, [0.0, 0.0], 1.0);
    s.execute(
        "bspec.set",
        &json!({"replace": true, "spec": {
            "levels": [
                {"name": "L1", "elevation_ft": 0, "height_ft": 12},
                {"name": "L2", "elevation_ft": 12, "height_ft": 12, "sheet": file.to_string_lossy()},
                {"name": "L3", "elevation_ft": 24, "height_ft": 12, "sheet": dir.join("missing.dxf").to_string_lossy()}
            ],
            "layers": [{"layer": "A-WALL", "role": "wall"}]
        }}),
    )
    .unwrap();
    let d = s.execute("grids.detect", &json!({"level": "L2"})).unwrap();
    let l2 = level(&d, "L2");
    assert_eq!((labels(l2), l2["sheet"].clone()), (vec!["1".to_string(), "2".into(), "A".into(), "B".into()], json!(file.to_string_lossy())), "{l2}");
    assert_eq!(l2["intersections"][0], json!({"label": "A-1", "at": [700.0, 300.0]}), "{l2}");
    let r = s.execute("levels.align", &json!({"apply": true})).unwrap();
    let l2 = level(&r, "L2");
    assert_eq!(
        (l2["matched"].clone(), l2["scale"].clone(), l2["offset"].clone(), l2["residual"].clone(), l2["base"].clone()),
        (json!(4), json!(1.0), json!([-700.0, -300.0]), json!(0.0), json!([700.0, 300.0])),
        "{l2}"
    );
    let l3 = level(&r, "L3");
    assert!(l3["note"].as_str().unwrap().starts_with("its sheet ") && l3["note"].as_str().unwrap().contains("could not be read"), "{l3}");
    let sp = s.execute("bspec.get", &json!({})).unwrap()["spec"].clone();
    assert_eq!(
        (sp["levels"][0]["base"].clone(), sp["levels"][1]["base"].clone(), sp["levels"][2]["base"].clone()),
        (json!([0.0, 0.0]), json!([700.0, 300.0]), Value::Null)
    );
    // Its columns are read in its own drawing.
    let e = s.execute("grids.columns", &json!({"level": "L2"})).unwrap_err().to_string();
    assert!(e.contains("own sheet"), "{e}");
    let _ = std::fs::remove_dir_all(&dir);
}
