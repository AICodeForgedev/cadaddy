//! Verifying a model built from plans: each `model.issues` kind produced by a deliberately broken
//! plan or spec, `model.render` images (whole model, a level, a room, a camera pose),
//! `model.overlay` differences and `model.counts` per level.

use std::sync::Arc;

use cadaddy_engine::Session;
use cadaddy_engine::doc::{Attrib, AttribModes, AttribProps, Block, Entity, EntityKind, Insert, Line, Text};
use cadaddy_engine::geom::Vec3;
use serde_json::{Value, json};

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

/// Feet → inches.
fn rect(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    let p = |x: f64, y: f64| [x * 12.0, y * 12.0];
    s.execute("pline", &json!({"vertices": [p(a[0], a[1]), p(b[0], a[1]), p(b[0], b[1]), p(a[0], b[1])], "closed": true})).unwrap();
}

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("line", &json!({"points": [[a[0] * 12.0, a[1] * 12.0], [b[0] * 12.0, b[1] * 12.0]]})).unwrap();
}

fn text(s: &mut Session, at: [f64; 2], t: &str) {
    s.execute("text", &json!({"at": [at[0] * 12.0, at[1] * 12.0], "text": t, "height": 6})).unwrap();
}

/// A block reference at `at` (feet) on `layer` with attributes.
fn place(s: Session, block: &str, layer: &str, at: [f64; 2], attrs: &[(&str, &str)]) -> Session {
    let mut d = (*s.doc().unwrap()).clone();
    let style = d.model.iter().find_map(|e| if let EntityKind::Text(t) = &e.kind { Some(t.clone()) } else { None }).unwrap();
    if !d.blocks.contains_key(block) {
        let mut b = Block::new(block);
        let h = d.new_handle();
        b.entities.push(Entity::new(h, EntityKind::Line(Line { a: Vec3::new(-18.0, 0.0, 0.0), b: Vec3::new(18.0, 0.0, 0.0) })));
        d.blocks.insert(block.into(), Arc::new(b));
    }
    let attribs = attrs
        .iter()
        .map(|(t, v)| Attrib {
            tag: (*t).into(),
            text: Text { value: (*v).into(), ..style.clone() },
            invisible: true,
            constant: false,
            prompt: String::new(),
            props: AttribProps::default(),
            multiline: None,
            modes: AttribModes::default(),
        })
        .collect();
    let ins = Insert {
        block: block.into(),
        insert: Vec3::new(at[0] * 12.0, at[1] * 12.0, 0.0),
        scale: Vec3::new(1.0, 1.0, 1.0),
        rotation: 0.0,
        attribs,
        cols: 1,
        rows: 1,
        col_spacing: 0.0,
        row_spacing: 0.0,
        clip: None,
    };
    let h = d.new_handle();
    let mut e = Entity::new(h, EntityKind::Insert(ins));
    e.common.layer = layer.into();
    d.model.push(e);
    let mut s = Session::empty();
    s.open_drawing(d, "Plan", None);
    s
}

/// A 30' × 12' building of 6" walls, OFFICE and STORAGE split at x = 15' by a centreline
/// partition, a window block in the south wall.
fn base(s: &mut Session, partition_to: f64) {
    layer(s, "A-WALL");
    rect(s, [0.0, 0.0], [30.0, 12.0]);
    rect(s, [0.5, 0.5], [29.5, 11.5]);
    layer(s, "A-WALL-CL");
    line(s, [15.0, 0.5], [15.0, partition_to]);
    layer(s, "A-ROOM-TAG");
    text(s, [5.0, 6.0], "OFFICE");
    text(s, [20.0, 6.0], "STORAGE");
}

fn spec() -> Value {
    json!({
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "base": [0, 0]}],
        "layers": [
            {"layer": "A-WALL-CL", "role": "wall", "wall_type": "Partition"},
            {"layer": "A-WALL-LOW", "role": "wall", "wall_type": "Curb"},
            {"layer": "A-WALL", "role": "wall", "wall_type": "Exterior"},
            {"layer": "A-WIN", "role": "window"},
            {"layer": "E-LITE", "role": "fixture"},
            {"layer": "A-ROOM-TAG", "role": "room_tag"}
        ],
        "wall_types": [
            {"name": "Exterior", "thickness_in": 6},
            {"name": "Partition", "thickness_in": 4.875, "from": "centerline"},
            {"name": "Curb", "height_ft": 0.5, "thickness_in": 6, "from": "centerline"}
        ],
        "rooms": [{"name": "Office"}, {"name": "Storage", "ceiling_ft": 7}],
        "families": [{"name": "Window", "kind": "window", "block": "W*", "sill_ft": 3, "head_ft": 7}]
    })
}

/// The good building, built.
fn good() -> Session {
    let mut s = Session::new();
    base(&mut s, 11.5);
    let mut s = place(s, "W36", "A-WIN", [8.0, 0.25], &[]);
    s.execute("bspec.set", &json!({"spec": spec(), "replace": true})).unwrap();
    s
}

fn kinds(r: &Value) -> Vec<String> {
    r["issues"].as_array().unwrap().iter().map(|i| i["kind"].as_str().unwrap().to_string()).collect()
}

fn of_kind<'a>(r: &'a Value, k: &str) -> Vec<&'a Value> {
    r["issues"].as_array().unwrap().iter().filter(|i| i["kind"] == k).collect()
}

fn handle_layer(s: &Session, h: &Value) -> String {
    let h = h.as_str().unwrap();
    s.doc().unwrap().model.iter().find(|e| e.handle.hex() == h).map(|e| e.common.layer.clone()).unwrap_or_default().to_string()
}

#[test]
fn a_good_building_has_no_issues_until_built_and_none_after() {
    let mut s = good();
    let r = s.execute("model.issues", &json!({})).unwrap();
    assert_eq!(kinds(&r), vec!["not_built"], "{r}");
    s.execute("model.build", &json!({})).unwrap();
    let r = s.execute("model.issues", &json!({})).unwrap();
    assert_eq!(r["total"], json!(0), "{r}");
    let c = s.execute("model.counts", &json!({})).unwrap();
    assert_eq!(c["match"], json!(true), "{c}");
    assert_eq!(c["levels"][0]["plan"]["windows"], json!(1));
    assert_eq!(c["levels"][0]["model"]["windows"], json!(1));
}

#[test]
fn each_issue_kind_from_a_broken_model() {
    let mut s = Session::new();
    // The partition stops short: OFFICE and STORAGE are one space.
    base(&mut s, 8.0);
    // A room tag outside the building: no closed outline.
    text(&mut s, [40.0, 6.0], "LOBBY");
    // Two partitions a hair apart (overlapping walls) and a 6" curb (a wall without height).
    layer(&mut s, "A-WALL-CL");
    line(&mut s, [22.0, 0.5], [22.0, 11.5]);
    line(&mut s, [22.1, 0.5], [22.1, 11.5]);
    layer(&mut s, "A-WALL-LOW");
    line(&mut s, [2.0, 3.0], [6.0, 3.0]);
    // A window block in the middle of a room (not in any wall).
    let s = place(s, "W36", "A-WIN", [8.0, 6.0], &[]);
    // Ceiling fixtures: one in STORAGE, whose 7' ceiling is under the level's 9' (above the
    // ceiling), and one at -12" (below the floor).
    let s = place(s, "LT1", "E-LITE", [18.0, 9.0], &[]);
    let mut s = place(s, "LT1", "E-LITE", [5.0, 9.0], &[("AFF", "-12")]);
    let mut sp = spec();
    // A second level with nothing on its region, starting 5' above the first's top.
    sp["levels"][0]["region"] = json!([-120, -120, 600, 240]);
    sp["levels"].as_array_mut().unwrap().push(json!({"name": "L2", "elevation_ft": 15, "height_ft": 10, "region": [0, 1200, 360, 1400]}));
    s.execute("bspec.set", &json!({"spec": sp, "replace": true})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    let r = s.execute("model.issues", &json!({})).unwrap();
    let k = kinds(&r);
    for want in
        ["leak", "opening_not_in_wall", "wall_without_height", "walls_overlap", "fixture_above_ceiling", "fixture_below_floor", "level_unmatched"]
    {
        assert!(k.iter().any(|x| x == want), "{want} missing: {r}");
    }
    // Leaks: the open LOBBY, and OFFICE running into STORAGE (both tags named).
    let leaks = of_kind(&r, "leak");
    assert!(leaks.iter().any(|l| l["message"].as_str().unwrap().starts_with("LOBBY: no closed outline")), "{leaks:?}");
    let joined = leaks.iter().find(|l| l["handles"].as_array().unwrap().len() == 2).expect("two rooms joined");
    assert!(joined["message"].as_str().unwrap().contains("runs into"));
    // The floating window names its block reference.
    let o = of_kind(&r, "opening_not_in_wall");
    assert_eq!(o.len(), 1, "{o:?}");
    assert_eq!(handle_layer(&s, &o[0]["handles"][0]), "A-WIN");
    // The curb: a wall 6" high, from the curb line.
    let low = of_kind(&r, "wall_without_height");
    assert_eq!(low.len(), 1, "{low:?}");
    assert_eq!(handle_layer(&s, &low[0]["handles"][1]), "A-WALL-LOW");
    // The two partitions overlap; the corners and tees of the good walls don't count.
    let ov = of_kind(&r, "walls_overlap");
    assert_eq!(ov.len(), 1, "{ov:?}");
    // The fixtures.
    let up = of_kind(&r, "fixture_above_ceiling");
    assert_eq!(up.len(), 1, "{up:?}");
    assert!(up[0]["message"].as_str().unwrap().contains("in STORAGE"), "{up:?}");
    assert_eq!(of_kind(&r, "fixture_below_floor").len(), 1);
    // Levels: L2 has nothing on it and starts 5' above L1's top.
    let lv: Vec<&str> = of_kind(&r, "level_unmatched").iter().map(|i| i["message"].as_str().unwrap()).collect();
    assert!(lv.iter().any(|m| m.contains("L2 has no plan objects")), "{lv:?}");
    assert!(lv.iter().any(|m| m.contains("L2 starts 5'-0\" above the top of L1")), "{lv:?}");
    // Filter by kind.
    let only = s.execute("model.issues", &json!({"kind": "leak"})).unwrap();
    assert_eq!(only["total"], json!(leaks.len()));
    // Objects built for a level the spec no longer has.
    s.execute("bspec.set", &json!({"spec": {"levels": [{"name": "L1", "remove": true}, {"name": "Podium", "elevation_ft": 0, "height_ft": 10}]}}))
        .unwrap();
    let r = s.execute("model.issues", &json!({"kind": "level_unmatched"})).unwrap();
    assert!(r["issues"].as_array().unwrap().iter().any(|i| i["message"].as_str().unwrap().contains("built for level L1")), "{r}");
    // Counts: two window blocks in the plan, one built.
    let c = s.execute("model.counts", &json!({})).unwrap();
    assert_eq!(c["match"], json!(false), "{c}");
    issues_come_a_page_at_a_time(&mut s);
}

#[test]
fn renders_overlay_and_counts() {
    let dir = std::env::temp_dir().join(format!("cadaddy-verify3d-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut s = good();
    assert!(s.execute("model.render", &json!({})).is_err(), "nothing built yet");
    s.execute("model.build", &json!({})).unwrap();
    let png = |r: &Value| -> Vec<u8> {
        match r["path"].as_str() {
            Some(p) => std::fs::read(p).unwrap(),
            None => vec![0x89],
        }
    };
    for (name, params) in [
        ("model", json!({})),
        ("level", json!({"level": "L1", "style": "shaded"})),
        ("room", json!({"room": "Office"})),
        ("pose", json!({"eye": [60, 72, 60], "target": [300, 72, 60], "width": 320, "height": 240})),
    ] {
        let path = dir.join(format!("{name}.png"));
        let mut p = params.clone();
        p["path"] = json!(path.to_string_lossy());
        let r = s.execute("model.render", &p).unwrap();
        let bytes = png(&r);
        assert_eq!(&bytes[..4], b"\x89PNG", "{name}");
        assert!(r["triangles"].as_u64().unwrap() > 0, "{name}: {r}");
    }
    // The room keeps fewer triangles than the whole model; the pose only what is ahead of it.
    let all = s.execute("model.render", &json!({"width": 64, "height": 64})).unwrap()["triangles"].as_u64().unwrap();
    let room = s.execute("model.render", &json!({"room": "Office", "width": 64, "height": 64})).unwrap()["triangles"].as_u64().unwrap();
    assert!(room < all, "{room} < {all}");
    assert!(s.execute("model.render", &json!({"width": 64, "height": 64})).unwrap()["png"].as_str().is_some_and(|b| !b.is_empty()));
    assert!(s.execute("model.render", &json!({"room": "Nowhere"})).is_err());
    assert!(s.execute("model.render", &json!({"level": "L9"})).is_err());
    assert!(s.execute("model.render", &json!({"eye": [0, 0, 0]})).is_err());
    assert!(s.execute("model.render", &json!({"eye": "x", "target": [0, 0, 0]})).is_err());
    // Overlay of the good building: no differences.
    let path = dir.join("overlay.png");
    let r = s.execute("model.overlay", &json!({"path": path.to_string_lossy()})).unwrap();
    assert_eq!(r["differences"], json!([]), "{r}");
    assert!(r["footprints"].as_u64().unwrap() >= 6, "{r}");
    assert_eq!(&png(&r)[..4], b"\x89PNG");
    // A lone wall line (no partner face) and a window in mid-room show as differences.
    layer(&mut s, "A-WALL");
    line(&mut s, [0.0, -10.0], [10.0, -10.0]);
    let mut s = place(s, "W36", "A-WIN", [8.0, 6.0], &[]);
    s.execute("model.build", &json!({})).unwrap();
    let r = s.execute("model.overlay", &json!({})).unwrap();
    let k: Vec<&str> = r["differences"].as_array().unwrap().iter().map(|d| d["kind"].as_str().unwrap()).collect();
    assert!(k.contains(&"wall_not_built") && k.contains(&"opening_not_built"), "{r}");
    assert!(!k.contains(&"wall_not_in_plan"), "{r}");
    let c = s.execute("model.counts", &json!({})).unwrap();
    assert_eq!(c["levels"][0]["plan"]["windows"], json!(2), "{c}");
    assert_eq!(c["levels"][0]["model"]["windows"], json!(1), "{c}");
    assert_eq!(c["match"], json!(false));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A big model's issues come a page at a time: `total` and `byKind` count them all, `next`
/// walks the rest, and no issue is skipped or repeated.
fn issues_come_a_page_at_a_time(s: &mut Session) {
    let all = s.execute("model.issues", &json!({ "limit": 1000 })).unwrap();
    let total = all["total"].as_u64().unwrap() as usize;
    assert!(total >= 2, "{all}");
    assert!(all.get("next").is_none());
    let mut seen = Vec::new();
    let mut cursor = json!(null);
    for _ in 0..total + 1 {
        let r = s.execute("model.issues", &json!({ "limit": 1, "cursor": cursor })).unwrap();
        assert_eq!(r["total"].as_u64(), Some(total as u64));
        assert_eq!(r["byKind"], all["byKind"]);
        seen.extend(r["issues"].as_array().unwrap().iter().cloned());
        match r.get("next") {
            Some(n) => cursor = n.clone(),
            None => break,
        }
    }
    assert_eq!(seen.as_slice(), all["issues"].as_array().unwrap().as_slice());
    // Hostile paging: never a panic, the limit clamped.
    for p in [json!({ "limit": 0 }), json!({ "limit": u64::MAX }), json!({ "cursor": u64::MAX }), json!({ "limit": -3, "cursor": "x" })] {
        let r = s.execute("model.issues", &p).unwrap();
        assert!(r["issues"].as_array().unwrap().len() <= 1000, "{p}");
    }
}

/// A 45' × 12' building of 6" walls in three rooms tagged by number only: 101 | 102 at x = 15'
/// (the partition up to `partition_to`) and 102 | 103 at x = 30' (full height). With
/// `west_open`, the west wall is drawn as lines with a 4' gap in both faces.
fn numbered(partition_to: f64, west_open: bool) -> Session {
    let mut s = Session::new();
    layer(&mut s, "A-WALL");
    if west_open {
        for (a, b) in [
            ([0.0, 0.0], [45.0, 0.0]),
            ([45.0, 0.0], [45.0, 12.0]),
            ([45.0, 12.0], [0.0, 12.0]),
            ([0.0, 12.0], [0.0, 8.0]),
            ([0.0, 4.0], [0.0, 0.0]),
            ([0.5, 0.5], [44.5, 0.5]),
            ([44.5, 0.5], [44.5, 11.5]),
            ([44.5, 11.5], [0.5, 11.5]),
            ([0.5, 11.5], [0.5, 8.0]),
            ([0.5, 4.0], [0.5, 0.5]),
            ([0.0, 8.0], [0.5, 8.0]),
            ([0.0, 4.0], [0.5, 4.0]),
        ] {
            line(&mut s, a, b);
        }
        // The gap's jambs close the wall ends, not the room: no line across the opening.
    } else {
        rect(&mut s, [0.0, 0.0], [45.0, 12.0]);
        rect(&mut s, [0.5, 0.5], [44.5, 11.5]);
    }
    layer(&mut s, "A-WALL-CL");
    line(&mut s, [15.0, 0.5], [15.0, partition_to]);
    line(&mut s, [30.0, 0.5], [30.0, 11.5]);
    layer(&mut s, "A-ROOM-TAG");
    text(&mut s, [7.0, 6.0], "101");
    text(&mut s, [22.0, 6.0], "102");
    text(&mut s, [37.0, 6.0], "103");
    s.execute("bspec.set", &json!({"spec": spec(), "replace": true})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    s
}

fn tag_text(s: &Session, h: &Value) -> String {
    let h = h.as_str().unwrap();
    s.doc()
        .unwrap()
        .model
        .iter()
        .find(|e| e.handle.hex() == h)
        .and_then(|e| if let EntityKind::Text(t) = &e.kind { Some(t.value.clone()) } else { None })
        .unwrap_or_default()
}

#[test]
fn rooms_tagged_by_number_only_are_checked_for_leaks() {
    // Closed rooms: no leak.
    let mut s = numbered(11.5, false);
    let r = s.execute("model.issues", &json!({"kind": "leak"})).unwrap();
    assert_eq!(r["total"], json!(0), "{r}");

    // The 101 | 102 partition stops short: 101 runs into 102, and 103 (closed) is never named.
    let mut s = numbered(8.0, false);
    let r = s.execute("model.issues", &json!({"kind": "leak"})).unwrap();
    assert_eq!(r["total"], json!(1), "{r}");
    assert_eq!(r["byKind"]["leak"], json!(1), "{r}");
    let leaks = of_kind(&r, "leak");
    assert_eq!(leaks.len(), 1, "{r}");
    let m = leaks[0]["message"].as_str().unwrap();
    assert!(m.contains("101") && m.contains("runs into") && m.contains("102"), "{m}");
    assert!(!m.contains("103"), "{m}");
    let tags: Vec<String> = leaks[0]["handles"].as_array().unwrap().iter().map(|h| tag_text(&s, h)).collect();
    assert_eq!(tags.len(), 2, "{tags:?}");
    assert!(tags.contains(&"101".to_string()) && tags.contains(&"102".to_string()), "{tags:?}");

    // 101's west wall has a gap to the outside: no closed outline around 101, named by its number;
    // 102 and 103 are closed.
    let mut s = numbered(11.5, true);
    let r = s.execute("model.issues", &json!({"kind": "leak"})).unwrap();
    assert_eq!(r["total"], json!(1), "{r}");
    let leaks = of_kind(&r, "leak");
    assert!(leaks[0]["message"].as_str().unwrap().starts_with("101: no closed outline"), "{r}");
    assert_eq!(leaks[0]["handles"].as_array().unwrap().len(), 1, "{r}");
    assert_eq!(tag_text(&s, &leaks[0]["handles"][0]), "101");
    assert!(leaks.iter().all(|l| !l["message"].as_str().unwrap().contains("103")), "{r}");
}

#[test]
fn a_number_tag_beside_a_name_tag_is_that_rooms_number_not_another_room() {
    // OFFICE 101 and STORAGE 102, closed: no leak.
    let mut s = Session::new();
    base(&mut s, 11.5);
    text(&mut s, [5.0, 4.0], "101");
    text(&mut s, [20.0, 4.0], "102");
    s.execute("bspec.set", &json!({"spec": spec(), "replace": true})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    let r = s.execute("model.issues", &json!({})).unwrap();
    assert_eq!(r["total"], json!(0), "{r}");

    // Joined: one leak (the names), not a second one for their numbers.
    let mut s = Session::new();
    base(&mut s, 8.0);
    text(&mut s, [5.0, 4.0], "101");
    text(&mut s, [20.0, 4.0], "102");
    s.execute("bspec.set", &json!({"spec": spec(), "replace": true})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    let r = s.execute("model.issues", &json!({"kind": "leak"})).unwrap();
    assert_eq!(r["total"], json!(1), "{r}");
    let m = of_kind(&r, "leak")[0]["message"].as_str().unwrap().to_string();
    assert!(m.contains("OFFICE") && m.contains("STORAGE") && m.contains("runs into"), "{m}");
}
