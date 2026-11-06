//! Perception tools for agents on synthetic plans with known answers: wall pairs and their
//! thickness, door swings and window blocks, room outlines with their names and numbers, notes
//! (ceiling heights, scales, finish codes, levels), sheets from title blocks and text, layer
//! summaries with PNG crops, and paging.

use cadaddy_engine::Session;
use serde_json::{Value, json};

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("line", &json!({"points": [a, b]})).unwrap();
}

fn rect(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("pline", &json!({"vertices": [a, [b[0], a[1]], b, [a[0], b[1]]], "closed": true})).unwrap();
}

fn text(s: &mut Session, at: [f64; 2], t: &str) {
    s.execute("text", &json!({"at": at, "text": t, "height": 6})).unwrap();
}

/// Two 19' × 19' rooms (OFFICE 101, LOBBY 102) inside 6" outer walls, a 12" wall between them.
fn plan() -> Session {
    let mut s = Session::new();
    s.execute("layer.new", &json!({"name": "A-WALL", "current": true})).unwrap();
    rect(&mut s, [0.0, 0.0], [480.0, 240.0]);
    rect(&mut s, [6.0, 6.0], [234.0, 234.0]);
    rect(&mut s, [246.0, 6.0], [474.0, 234.0]);
    s.execute("layer.new", &json!({"name": "A-ANNO", "current": true})).unwrap();
    text(&mut s, [100.0, 120.0], "OFFICE");
    text(&mut s, [100.0, 100.0], "101");
    text(&mut s, [340.0, 120.0], "LOBBY");
    text(&mut s, [340.0, 100.0], "102");
    s
}

fn items(r: &Value) -> Vec<Value> {
    r["items"].as_array().unwrap().clone()
}

#[test]
fn rooms_are_the_outlines_around_their_names_with_their_numbers() {
    let mut s = plan();
    let r = s.execute("plan.rooms", &json!({})).unwrap();
    let rooms = items(&r);
    assert_eq!(rooms.len(), 2, "{r}");
    let office = rooms.iter().find(|x| x["name"] == "OFFICE").unwrap();
    assert_eq!(office["number"], "101");
    assert!((office["area"].as_f64().unwrap() - 228.0 * 228.0).abs() < 1.0, "{office}");
    assert!((office["perimeter"].as_f64().unwrap() - 4.0 * 228.0).abs() < 1.0);
    assert!(office["confidence"].as_f64().unwrap() >= 0.9);
    assert!(office["outline"].as_array().unwrap().len() >= 4);
    let lobby = rooms.iter().find(|x| x["name"] == "LOBBY").unwrap();
    assert_eq!(lobby["number"], "102");
    // Without outlines (cheaper), paged one at a time.
    let one = s.execute("plan.rooms", &json!({"outline": false, "limit": 1})).unwrap();
    assert_eq!((one["items"].as_array().unwrap().len(), one["total"].as_u64(), one["next"].as_u64()), (1, Some(2), Some(1)));
    assert!(one["items"][0].get("outline").is_none());
}

#[test]
fn walls_are_parallel_face_pairs_a_wall_apart() {
    let mut s = plan();
    let r = s.execute("plan.walls", &json!({"limit": 200})).unwrap();
    let walls = items(&r);
    let near = |t: f64, l: f64| walls.iter().any(|w| (w["thickness"].as_f64().unwrap() - t).abs() < 1e-6 && w["length"].as_f64().unwrap() >= l);
    assert!(near(6.0, 200.0), "the 6\" outer walls: {walls:?}");
    assert!(near(12.0, 228.0), "the 12\" wall between the rooms");
    // All on A-WALL, centrelines between the faces.
    assert!(walls.iter().all(|w| w["layer"] == "A-WALL"));
    let mid = walls.iter().find(|w| (w["thickness"].as_f64().unwrap() - 12.0).abs() < 1e-6).unwrap();
    assert!((mid["centerline"][0][0].as_f64().unwrap() - 240.0).abs() < 1e-6, "{mid}");
    // Limits: nothing as thin as 1".
    let thin = s.execute("plan.walls", &json!({"minThickness": 0.1, "maxThickness": 1})).unwrap();
    assert_eq!(thin["total"], 0);
    // Only the layers asked for.
    assert_eq!(s.execute("plan.walls", &json!({"layers": "A-ANNO"})).unwrap()["total"], 0);
}

#[test]
fn openings_are_door_swings_and_door_or_window_blocks() {
    let mut s = Session::new();
    // A 36" door hinged at (100, 0): leaf up to (100, 36), swing to (136, 0).
    s.execute("arc", &json!({"center": [100, 0], "radius": 36, "start": 0, "end": 90})).unwrap();
    line(&mut s, [100.0, 0.0], [100.0, 36.0]);
    // A small arc (a detail) is not a door.
    s.execute("arc", &json!({"center": [0, 0], "radius": 4, "start": 0, "end": 90})).unwrap();
    // A window block.
    let w = s.execute("line", &json!({"points": [[0, 0], [36, 0]]})).unwrap();
    let hs = vec![w["handles"][0].as_str().unwrap().to_string()];
    s.execute("block", &json!({"name": "WINDOW-36", "base": [0, 0], "handles": hs, "keep": "delete"})).unwrap();
    s.execute("insert", &json!({"name": "WINDOW-36", "at": [300, 0]})).unwrap();
    let r = s.execute("plan.openings", &json!({})).unwrap();
    let o = items(&r);
    assert_eq!(o.len(), 2, "{r}");
    let door = o.iter().find(|x| x["kind"] == "door").unwrap();
    assert_eq!((door["width"].as_f64(), door["swing"].as_f64()), (Some(36.0), Some(90.0)));
    assert!(door["confidence"].as_f64().unwrap() >= 0.8, "the leaf is there: {door}");
    let win = o.iter().find(|x| x["kind"] == "window").unwrap();
    assert_eq!((win["width"].as_f64(), win["at"][0].as_f64()), (Some(36.0), Some(300.0)));
}

#[test]
fn notes_find_ceilings_scales_finishes_and_levels() {
    let mut s = Session::new();
    for (i, t) in
        ["CLG 9'-0\" AFF", "SCALE: 1/4\" = 1'-0\"", "FLOOR: CPT-1, BASE RB-2", "LEVEL 2 EL. 112'-0\"", "SEE A-101", "NOT TO SCALE"].iter().enumerate()
    {
        text(&mut s, [0.0, 20.0 * i as f64], t);
    }
    let r = s.execute("plan.notes", &json!({"limit": 50})).unwrap();
    let n = items(&r);
    let has = |k: &str, v: Value| n.iter().any(|x| x["kind"] == k && x["value"] == v);
    assert!(has("ceiling", json!(108.0)), "{n:?}");
    assert!(has("scale", json!("1/4\" = 1'-0\"")));
    assert!(has("scale", json!("NTS")));
    assert!(has("finish", json!("CPT-1")) && has("finish", json!("RB-2")));
    assert!(has("level", json!(1344.0)));
    assert!(!n.iter().any(|x| x["value"] == "A-101"), "a sheet reference is not a finish code");
    assert!(s.execute("plan.notes", &json!({"kind": "finish"})).unwrap()["items"].as_array().unwrap().iter().all(|x| x["kind"] == "finish"));
}

#[test]
fn sheets_come_from_title_blocks_and_sheet_number_text() {
    let mut s = Session::new();
    // A title block with attributes, inserted on Layout1.
    s.execute("attdef", &json!({"tag": "SHEET_NO", "default": "", "at": [0, 0], "height": 0.2})).unwrap();
    s.execute("attdef", &json!({"tag": "TITLE", "default": "", "at": [0, 1], "height": 0.2})).unwrap();
    rect(&mut s, [0.0, 0.0], [10.0, 2.0]);
    let hs: Vec<String> = s.doc().unwrap().model.iter().map(|e| e.handle.hex()).collect();
    s.execute("block", &json!({"name": "TB", "base": [0, 0], "handles": hs, "keep": "delete"})).unwrap();
    s.execute("layout.set", &json!({"name": "Layout1"})).unwrap();
    s.execute("insert", &json!({"name": "TB", "at": [0, 0], "attribs": {"SHEET_NO": "A101", "TITLE": "FLOOR PLAN"}})).unwrap();
    let r = s.execute("plan.sheets", &json!({})).unwrap();
    let sh = items(&r);
    let tb = sh.iter().find(|x| x["number"] == "A101").unwrap_or_else(|| panic!("{r}"));
    assert_eq!(tb["layout"], "Layout1", "{r}");
    assert_eq!((tb["title"].as_str(), tb["from"].as_str()), (Some("FLOOR PLAN"), Some("attribute")));
    assert_eq!(tb["confidence"], 0.95);
}

#[test]
fn sheet_numbers_fall_back_to_the_largest_sheet_number_text() {
    let mut s = Session::new();
    // A layout with no title block: a large "A201", a small sheet reference and a note.
    s.execute("layout", &json!({"option": "new", "name": "Plans"})).unwrap();
    s.execute("layout.set", &json!({"name": "Plans"})).unwrap();
    s.execute("text", &json!({"at": [9, 0.5], "text": "A201", "height": 0.5})).unwrap();
    s.execute("text", &json!({"at": [1, 1], "text": "A501", "height": 0.1})).unwrap();
    s.execute("text", &json!({"at": [1, 2], "text": "FLOOR PLAN", "height": 0.3})).unwrap();
    let r = s.execute("plan.sheets", &json!({})).unwrap();
    let sh = items(&r);
    let p = sh.iter().find(|x| x["layout"] == "Plans").unwrap_or_else(|| panic!("{r}"));
    assert_eq!((p["number"].as_str(), p["from"].as_str(), p["title"].as_str()), (Some("A201"), Some("text"), None), "{p}");
    assert_eq!(p["confidence"], 0.6);
}

#[test]
fn layers_summarise_counts_pairs_and_write_crops() {
    let mut s = plan();
    let dir = std::env::temp_dir().join(format!("cadaddy-perceive-{}", std::process::id()));
    let r = s.execute("plan.layers", &json!({"crops": dir.to_str().unwrap()})).unwrap();
    let l = items(&r);
    let wall = l.iter().find(|x| x["layer"] == "A-WALL").unwrap();
    assert_eq!(wall["counts"]["Polyline"], 3);
    assert_eq!(wall["segments"], 12);
    assert!(wall["pairs"].as_u64().unwrap() > 0 && wall["pairSpacing"].as_f64().is_some(), "{wall}");
    // Every A-WALL face pairs up, on a WALL layer.
    assert_eq!(wall["confidence"], 1.0, "{wall}");
    let crop = wall["crop"].as_str().unwrap();
    let png = std::fs::read(crop).unwrap();
    assert_eq!(&png[1..4], b"PNG");
    let anno = l.iter().find(|x| x["layer"] == "A-ANNO").unwrap();
    assert_eq!(anno["counts"]["Text"], 4);
    assert_eq!(anno["confidence"], 0.0, "texts only: {anno}");
    let _ = std::fs::remove_dir_all(&dir);
    // Half of a layer's faces pair up, not on a WALL layer: ¾ × ½.
    s.execute("layer.new", &json!({"name": "X-MISC", "current": true})).unwrap();
    line(&mut s, [0.0, 400.0], [100.0, 400.0]);
    line(&mut s, [0.0, 405.0], [100.0, 405.0]);
    line(&mut s, [0.0, 500.0], [30.0, 560.0]);
    line(&mut s, [200.0, 500.0], [260.0, 520.0]);
    let r = s.execute("plan.layers", &json!({})).unwrap();
    let misc = items(&r).into_iter().find(|x| x["layer"] == "X-MISC").unwrap();
    assert_eq!((misc["segments"].as_u64(), misc["pairs"].as_u64(), misc["confidence"].as_f64()), (Some(4), Some(1), Some(0.375)), "{misc}");
    // Hostile paging.
    let r = s.execute("plan.layers", &json!({"offset": 1_000_000, "limit": 0})).unwrap();
    assert_eq!(r["items"].as_array().unwrap().len(), 0);
}

/// The walls of [`plan`] drawn as lines (each rectangle's edges counter-clockwise), turned `deg`
/// about the origin and moved to (1000, 500). With `flip`, one face of each wall is drawn the other way with a hair of
/// rise (+1e-9): a horizontal face right to left then leans to just under 180°.
fn line_plan(deg: f64, flip: bool) -> Session {
    let mut s = Session::new();
    s.execute("layer.new", &json!({"name": "A-WALL", "current": true})).unwrap();
    let (c, sn) = (deg.to_radians().cos(), deg.to_radians().sin());
    // Away from the origin: a face's offset with its sign flipped is then far from its partner's.
    let turn = |p: [f64; 2]| [p[0] * c - p[1] * sn + 1000.0, p[0] * sn + p[1] * c + 500.0];
    let edges = |a: [f64; 2], b: [f64; 2]| [(a, [b[0], a[1]]), ([b[0], a[1]], b), (b, [a[0], b[1]]), ([a[0], b[1]], a)];
    let outer = edges([0.0, 0.0], [480.0, 240.0]);
    let rooms = [edges([6.0, 6.0], [234.0, 234.0]), edges([246.0, 6.0], [474.0, 234.0])];
    // Flipped: the rooms' faces (one of each wall's two).
    for (a, b) in outer {
        line(&mut s, turn(a), turn(b));
    }
    for (a, b) in rooms.iter().flatten().copied() {
        let (a, b) = if flip { (b, [a[0], a[1] + 1e-9]) } else { (a, b) };
        line(&mut s, turn(a), turn(b));
    }
    s
}

/// A plan's walls as (thickness, length) to a thousandth, sorted.
fn wall_sizes(s: &mut Session) -> Vec<(i64, i64)> {
    let r = s.execute("plan.walls", &json!({"limit": 200})).unwrap();
    let mut v: Vec<(i64, i64)> = items(&r)
        .iter()
        .map(|w| ((w["thickness"].as_f64().unwrap() * 1000.0).round() as i64, (w["length"].as_f64().unwrap() * 1000.0).round() as i64))
        .collect();
    v.sort_unstable();
    v
}

#[test]
fn walls_pair_faces_drawn_either_way_and_turned() {
    let base = wall_sizes(&mut line_plan(0.0, false));
    // The polyline plan's walls, as lines: 6" outer walls and the 12" wall between the rooms.
    assert_eq!(base, wall_sizes(&mut plan()));
    assert!(base.contains(&(6000, 228_000)) && base.contains(&(12000, 228_000)), "{base:?}");
    // A face drawn right to left with a hair of rise is at just under 180°: it still pairs, the
    // same walls with the same lengths.
    assert_eq!(wall_sizes(&mut line_plan(0.0, true)), base, "flipped faces");
    // Turned 30°, both ways.
    assert_eq!(wall_sizes(&mut line_plan(30.0, false)), base, "turned 30°");
    assert_eq!(wall_sizes(&mut line_plan(30.0, true)), base, "turned 30°, flipped faces");
    // Nearly parallel faces either side of a half-degree mark (0.25° and 0.5°, either way of
    // drawing) are still a 6" wall.
    for mark in [0.25f64, 0.5, 90.5, 179.75] {
        for flip in [false, true] {
            let mut s = Session::new();
            s.execute("layer.new", &json!({"name": "A-WALL", "current": true})).unwrap();
            for (off, d) in [(0.0, -0.002), (6.0, 0.002)] {
                let a = (mark + d).to_radians();
                let (u, n) = ([a.cos(), a.sin()], [-mark.to_radians().sin(), mark.to_radians().cos()]);
                let m = [1000.0 + n[0] * off, 500.0 + n[1] * off];
                let (p, q) = ([m[0] - u[0] * 120.0, m[1] - u[1] * 120.0], [m[0] + u[0] * 120.0, m[1] + u[1] * 120.0]);
                if flip && off > 0.0 { line(&mut s, q, p) } else { line(&mut s, p, q) }
            }
            let r = s.execute("plan.walls", &json!({})).unwrap();
            let w = items(&r);
            assert_eq!(w.len(), 1, "mark {mark}°, flip {flip}: {r}");
            assert!((w[0]["thickness"].as_f64().unwrap() - 6.0).abs() < 1e-3, "{r}");
            assert!((w[0]["length"].as_f64().unwrap() - 240.0).abs() < 0.01, "{r}");
        }
    }
}

#[test]
fn rooms_are_named_by_their_name_not_by_notes_drawn_first() {
    // Notes inside the rooms drawn before their names and numbers: finish codes, a ceiling note,
    // levels, a scale and a sheet reference. Then the same labels in the other order.
    let notes = ["CPT-1", "CLG 9'-0\" AFF", "LEVEL 2", "SCALE: 1/4\" = 1'-0\"", "SEE A-501"];
    for reverse in [false, true] {
        let mut s = Session::new();
        s.execute("layer.new", &json!({"name": "A-WALL", "current": true})).unwrap();
        rect(&mut s, [0.0, 0.0], [480.0, 240.0]);
        rect(&mut s, [6.0, 6.0], [234.0, 234.0]);
        rect(&mut s, [246.0, 6.0], [474.0, 234.0]);
        s.execute("layer.new", &json!({"name": "A-ANNO", "current": true})).unwrap();
        let mut labels: Vec<([f64; 2], &str)> = notes.iter().enumerate().map(|(i, t)| ([20.0, 200.0 - 20.0 * i as f64], *t)).collect();
        // The other room's name has no room word, no better a name by its words than a floor
        // finish note or a level drawn before it.
        labels.extend([([260.0, 200.0], "FLOOR: VCT-2"), ([260.0, 180.0], "LEVEL 2 EL. 112'-0\"")]);
        labels.extend([([100.0, 120.0], "OFFICE"), ([100.0, 100.0], "101"), ([340.0, 120.0], "COPY"), ([340.0, 100.0], "102")]);
        if reverse {
            labels.reverse();
        }
        for (at, t) in labels {
            text(&mut s, at, t);
        }
        let r = s.execute("plan.rooms", &json!({"outline": false})).unwrap();
        let rooms = items(&r);
        let mut got: Vec<(String, String)> =
            rooms.iter().map(|x| (x["name"].as_str().unwrap().to_string(), x["number"].as_str().unwrap().to_string())).collect();
        got.sort();
        assert_eq!(got, [("COPY".to_string(), "102".to_string()), ("OFFICE".to_string(), "101".to_string())], "reverse {reverse}: {r}");
        let office = rooms.iter().find(|x| x["name"] == "OFFICE").unwrap();
        assert_eq!(office["area"], 228.0 * 228.0);
    }
}

#[test]
fn room_tag_blocks_name_and_number_their_rooms() {
    let mut s = Session::new();
    s.execute("layer.new", &json!({"name": "A-WALL", "current": true})).unwrap();
    rect(&mut s, [0.0, 0.0], [480.0, 240.0]);
    rect(&mut s, [6.0, 6.0], [234.0, 234.0]);
    rect(&mut s, [246.0, 6.0], [474.0, 234.0]);
    // Two room tag blocks: a box around a name and a number attribute, one with ROOM_NAME /
    // ROOM_NO tags, one with NAME / NUMBER.
    for (block, name_tag, no_tag) in [("RMTAG", "ROOM_NAME", "ROOM_NO"), ("TAG2", "NAME", "NUMBER")] {
        let layer = format!("TAGS-{block}");
        s.execute("layer.new", &json!({"name": layer, "current": true})).unwrap();
        s.execute("attdef", &json!({"tag": name_tag, "default": "", "at": [2, 8], "height": 6})).unwrap();
        s.execute("attdef", &json!({"tag": no_tag, "default": "", "at": [2, 1], "height": 6})).unwrap();
        rect(&mut s, [0.0, 0.0], [60.0, 16.0]);
        let hs: Vec<String> = s.doc().unwrap().model.iter().filter(|e| e.common.layer == layer.as_str()).map(|e| e.handle.hex()).collect();
        s.execute("block", &json!({"name": block, "base": [0, 0], "handles": hs, "keep": "delete"})).unwrap();
    }
    s.execute("layer.new", &json!({"name": "A-ANNO", "current": true})).unwrap();
    s.execute("insert", &json!({"name": "RMTAG", "at": [90, 100], "attribs": {"ROOM_NAME": "CONFERENCE", "ROOM_NO": "201"}})).unwrap();
    s.execute("insert", &json!({"name": "TAG2", "at": [330, 100], "attribs": {"NAME": "STAFF", "NUMBER": "202"}})).unwrap();
    let r = s.execute("plan.rooms", &json!({"outline": false})).unwrap();
    let rooms = items(&r);
    assert_eq!(rooms.len(), 2, "{r}");
    let conf = rooms.iter().find(|x| x["name"] == "CONFERENCE").unwrap_or_else(|| panic!("{r}"));
    assert_eq!((conf["number"].as_str(), conf["area"].as_f64()), (Some("201"), Some(228.0 * 228.0)), "{conf}");
    let staff = rooms.iter().find(|x| x["name"] == "STAFF").unwrap_or_else(|| panic!("{r}"));
    assert_eq!((staff["number"].as_str(), staff["area"].as_f64()), (Some("202"), Some(228.0 * 228.0)), "{staff}");
}
