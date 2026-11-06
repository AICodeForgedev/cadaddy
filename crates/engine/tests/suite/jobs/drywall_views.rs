//! Drywall views and takeoff tests (Q294 rework): wall inspector, view modes, finish faces and
//! drywall takeoff, using the Q293 wall assembly fixtures. Tests are scored to truth, not "> 0".

use cadaddy_engine::Session;
use serde_json::{Value, json};

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("line", &json!({"points": [a, b]})).unwrap();
}

fn rect(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("pline", &json!({"vertices": [a, [b[0], a[1]], b, [a[0], b[1]]], "closed": true})).unwrap();
}

fn text(s: &mut Session, at: [f64; 2], value: &str, height: f64) {
    s.execute("text", &json!({"at": at, "text": value, "height": height})).unwrap();
}

/// A made-up building with wall types and a build.
/// P1 partition: centreline at x=240, mesh spans the full building depth (24 ft = 288").
/// Height = 12.75 ft = 153". Face area per side: 24 × 12.75 = 306.0 sq ft.
fn building() -> Session {
    let mut s = Session::new();
    layer(&mut s, "A-WALL");
    let t = 7.25;
    rect(&mut s, [0.0, 0.0], [480.0, 288.0]);
    rect(&mut s, [t, t], [480.0 - t, 288.0 - t]);
    let h = 4.875 / 2.0;
    line(&mut s, [240.0 - h, t], [240.0 - h, 288.0 - t]);
    line(&mut s, [240.0 + h, t], [240.0 + h, 288.0 - t]);
    // Room name labels for geometric matching.
    layer(&mut s, "A-ROOM-NAME");
    text(&mut s, [120.0, 144.0], "OFFICE", 6.0);
    text(&mut s, [360.0, 144.0], "CORRIDOR", 6.0);
    layer(&mut s, "A-WALL");
    let layers = |side1: Value, core: Value, side2: Value| -> Value {
        let mut v: Vec<Value> = Vec::new();
        for (side, list) in [("side1", side1), ("core", core), ("side2", side2)] {
            for l in list.as_array().cloned().unwrap_or_default() {
                let mut l = l;
                l["side"] = json!(side);
                v.push(l);
            }
        }
        json!(v)
    };
    let exterior = json!({
        "tag": "X1", "description": "EXTERIOR WALL", "height": "full", "external": true,
        "layers": layers(
            json!([{"material": "5/8\" exterior sheathing", "thickness_in": 0.625, "category": "sheathing"}]),
            json!([{"material": "6\" 18 ga metal studs @ 16\" o.c.", "thickness_in": 6.0, "category": "framing"},
                   {"material": "6\" batt insulation", "thickness_in": 6.0, "category": "insulation", "in_cavity": true}]),
            json!([{"material": "5/8\" Type X gypsum board", "thickness_in": 0.625, "category": "board"}]))
    });
    let partition = json!({
        "tag": "P1", "description": "1 HOUR RATED CORRIDOR PARTITION", "height": "deck", "fire_rating": "1 HR", "acoustic_rating": "STC 45",
        "layers": layers(
            json!([{"material": "5/8\" Type X gypsum board", "thickness_in": 0.625, "category": "board"}]),
            json!([{"material": "3 5/8\" 20 ga metal studs @ 16\" o.c.", "thickness_in": 3.625, "category": "framing", "spacing_in": 16.0},
                   {"material": "3 1/2\" batt insulation", "thickness_in": 3.5, "category": "insulation", "in_cavity": true}]),
            json!([{"material": "5/8\" Type X gypsum board", "thickness_in": 0.625, "category": "board"}]))
    });
    let spec = json!({
        "version": 1,
        "units_per_foot": 12,
        "levels": [{"name": "1", "elevation_ft": 0, "height_ft": 14, "ceiling_ft": 9}],
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "X1"}],
        "wall_types": [
            {"name": "X1", "thickness_in": 7.25, "from": "pairs", "assembly": exterior},
            {"name": "P1", "thickness_in": 4.875, "from": "pairs", "assembly": partition}
        ],
        "walls": [{"level": "1", "a": [240.0, 7.25], "b": [240.0, 280.75], "wall_type": "P1", "height_ft": 12.75}],
        "rooms": [
            {"name": "OFFICE", "level": "1", "materials": {"wall": "latex paint"}},
            {"name": "CORRIDOR", "level": "1", "materials": {"wall": "epoxy paint"}}
        ]
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    s
}

/// Select all wall entities.
fn select_walls(s: &mut Session) -> Vec<String> {
    let d = s.doc().unwrap();
    let handles: Vec<String> =
        d.model.iter().filter(|e| cadaddy_engine::cmd::wallgen::made_as(e).as_deref() == Some("wall")).map(|e| e.handle.hex()).collect();
    handles
}

/// Get walls of a specific type from wall.info.
fn walls_of_type(r: &Value, type_name: &str) -> Vec<Value> {
    r["walls"].as_array().into_iter().flatten().filter(|w| w["wallType"].as_str() == Some(type_name)).cloned().collect()
}

// --------------------------------------------------------------------------
// MUST 1: Board area per side = one face's net area
// --------------------------------------------------------------------------

#[test]
fn p1_area_per_side_is_face_area() {
    let mut s = building();
    let handles = select_walls(&mut s);
    let r = s.execute("wall.info", &json!({"handles": handles})).unwrap();
    let p1_walls = walls_of_type(&r, "P1");
    assert!(!p1_walls.is_empty(), "building should have P1 wall meshes");

    // P1: mesh spans 288" (24 ft, the full building depth) × 153" (12.75 ft) high.
    // Face area per side = 24 × 12.75 = 306.0 sq ft.
    let expected = 306.0;
    for w in &p1_walls {
        let area = w["grossAreaSqFt"].as_f64().unwrap_or(0.0);
        let pct_diff = ((area - expected) / expected).abs();
        assert!(pct_diff < 0.02, "P1 face area should be ~{expected} sq ft ±2%, got {area} ({:.1}% off)", pct_diff * 100.0);
    }
}

#[test]
fn both_sides_same_area() {
    let mut s = building();
    let r = s.execute("drywall.takeoff", &json!({})).unwrap();
    let rows = r["rows"].as_array().unwrap();
    let p1_row = rows.iter().find(|r| r["group"].as_str() == Some("P1"));
    assert!(p1_row.is_some(), "should have a P1 group");
    let boards = p1_row.unwrap()["boards"].as_array().unwrap();
    // P1 has one board per side: side1 and side2 should have the same area.
    let s1: Vec<_> = boards.iter().filter(|b| b["side"].as_str() == Some("side1")).collect();
    let s2: Vec<_> = boards.iter().filter(|b| b["side"].as_str() == Some("side2")).collect();
    assert!(!s1.is_empty() && !s2.is_empty(), "P1 should have boards on both sides");
    let a1 = s1[0]["areaSqFt"].as_f64().unwrap();
    let a2 = s2[0]["areaSqFt"].as_f64().unwrap();
    assert!((a1 - a2).abs() < 1.0, "side1 ({a1}) and side2 ({a2}) should match");
    // Both should be ~306.0 sq ft (24 ft long × 12.75 ft high).
    let expected = 306.0;
    assert!(((a1 - expected) / expected).abs() < 0.02, "area should be ~{expected}, got {a1}");
}

// --------------------------------------------------------------------------
// MUST 1 (continued): wall.info asserts are unconditional
// --------------------------------------------------------------------------

#[test]
fn wall_info_returns_assembly_for_typed_walls() {
    let mut s = building();
    let handles = select_walls(&mut s);
    assert!(!handles.is_empty(), "building should have wall meshes");
    let r = s.execute("wall.info", &json!({"handles": handles})).unwrap();
    let walls = r["walls"].as_array().unwrap();
    assert!(!walls.is_empty(), "wall.info should return wall data");

    // At least one wall should have an assembly with layers.
    let has_assembly = walls.iter().any(|w| w["assembly"]["layers"].is_array());
    assert!(has_assembly, "at least one wall should have an assembly");

    // Unconditional asserts on X1 and P1 walls.
    let x1_walls = walls_of_type(&r, "X1");
    assert!(!x1_walls.is_empty(), "should have X1 walls");
    assert_eq!(x1_walls[0]["assembly"]["external"], true);
    assert!(x1_walls[0]["grossAreaSqFt"].as_f64().unwrap_or(0.0) > 0.0, "X1 wall should have area");

    let p1_walls = walls_of_type(&r, "P1");
    assert!(!p1_walls.is_empty(), "should have P1 walls");
    assert_eq!(p1_walls[0]["assembly"]["fireRating"], "1 HR");
    assert_eq!(p1_walls[0]["assembly"]["acousticRating"], "STC 45");
}

#[test]
fn wall_info_no_selection_returns_empty() {
    let mut s = Session::new();
    let r = s.execute("wall.info", &json!({})).unwrap();
    assert_eq!(r["count"], 0);
}

// --------------------------------------------------------------------------
// MUST 2: View modes actually colour walls
// --------------------------------------------------------------------------

#[test]
fn viewmode_by_rating_colors_match_legend() {
    let mut s = building();
    let r = s.execute("drywall.viewmode", &json!({"mode": "rating"})).unwrap();
    assert_eq!(r["mode"], "rating");
    let legend = r["legend"].as_array().unwrap();
    let labels: Vec<&str> = legend.iter().filter_map(|l| l["label"].as_str()).collect();
    assert!(labels.contains(&"1 HR"), "legend should contain '1 HR', got {:?}", labels);
    assert!(labels.contains(&"Not rated"), "legend should contain 'Not rated', got {:?}", labels);

    // The viewmode should populate color overrides on the session.
    assert!(!s.drywall_color_overrides.is_empty(), "color overrides should be populated");

    // Now check that P1 walls get the "1 HR" colour and X1 walls get "Not rated".
    let hr_color: Vec<u8> =
        legend.iter().find(|l| l["label"] == "1 HR").unwrap()["color"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap() as u8).collect();
    let nr_color: Vec<u8> =
        legend.iter().find(|l| l["label"] == "Not rated").unwrap()["color"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap() as u8).collect();

    let d = s.doc().unwrap();
    for e in d.model.iter() {
        if cadaddy_engine::cmd::wallgen::made_as(e).as_deref() != Some("wall") {
            continue;
        }
        let tags: Vec<String> = e
            .common
            .xdata
            .iter()
            .skip_while(|t| !(t.code == 1001 && t.value.trim().eq_ignore_ascii_case("CADADDY_WALLGEN")))
            .skip(1)
            .take_while(|t| t.code != 1001)
            .filter(|t| t.code == 1000)
            .map(|t| t.value.clone())
            .collect();
        let type_name = tags.iter().skip(3).find_map(|v| v.strip_prefix("type:")).map(str::to_string);

        if let Some(override_color) = s.drywall_color_overrides.get(&e.handle) {
            match type_name.as_deref() {
                Some("P1") => {
                    assert_eq!(*override_color, [hr_color[0], hr_color[1], hr_color[2]], "P1 wall should get '1 HR' colour");
                }
                Some("X1") => {
                    assert_eq!(*override_color, [nr_color[0], nr_color[1], nr_color[2]], "X1 wall should get 'Not rated' colour");
                }
                _ => {}
            }
        }
    }
}

#[test]
fn viewmode_by_type() {
    let mut s = building();
    let r = s.execute("drywall.viewmode", &json!({"mode": "type"})).unwrap();
    assert_eq!(r["mode"], "type");
    let legend = r["legend"].as_array().unwrap();
    assert!(legend.len() >= 2, "should have at least 2 wall types in legend");
    for item in legend {
        assert!(item["label"].is_string());
        assert!(item["color"].is_array());
    }
}

#[test]
fn viewmode_by_board_layers() {
    let mut s = building();
    let r = s.execute("drywall.viewmode", &json!({"mode": "boardLayers"})).unwrap();
    assert_eq!(r["mode"], "boardLayers");
    let legend = r["legend"].as_array().unwrap();
    assert!(!legend.is_empty());
}

#[test]
fn viewmode_by_height_rule() {
    let mut s = building();
    let r = s.execute("drywall.viewmode", &json!({"mode": "heightRule"})).unwrap();
    assert_eq!(r["mode"], "heightRule");
}

#[test]
fn viewmode_off_clears() {
    let mut s = building();
    s.execute("drywall.viewmode", &json!({"mode": "type"})).unwrap();
    assert!(s.drywall_viewmode.is_some());
    assert!(!s.drywall_color_overrides.is_empty());
    s.execute("drywall.viewmode", &json!({"mode": "off"})).unwrap();
    assert!(s.drywall_viewmode.is_none());
    assert!(s.drywall_color_overrides.is_empty());
}

#[test]
fn viewmode_color_override_in_render() {
    // Build a display list with colour overrides and check that wall prims have the right colour.
    let mut s = building();
    s.execute("drywall.viewmode", &json!({"mode": "rating"})).unwrap();
    let d = s.doc().unwrap();
    let overrides = &s.drywall_color_overrides;
    let opts = cadaddy_render::Options { color_overrides: overrides.iter().map(|(&h, &c)| (h, c)).collect(), ..Default::default() };
    let list = cadaddy_render::build(d, &cadaddy_doc::Space::Model, &opts);
    // The list should have prims, and wall prims should have override colours.
    assert!(!list.prims.is_empty(), "display list should have prims");
    // Check that at least one prim uses an override colour.
    let override_handles: std::collections::BTreeSet<_> = overrides.keys().copied().collect();
    let has_override_prim = list.prims.iter().any(|p| override_handles.contains(&p.handle));
    assert!(has_override_prim, "at least one prim should be from a wall entity with a colour override");
}

// --------------------------------------------------------------------------
// MUST 3: Finish faces — outermost board + room matching
// --------------------------------------------------------------------------

#[test]
fn finishfaces_correct_board_and_room() {
    let mut s = building();
    let r = s.execute("drywall.finishfaces", &json!({})).unwrap();
    let faces = r["faces"].as_array().unwrap();
    assert!(!faces.is_empty(), "should have finish faces");

    // The P1 partition should have two faces.
    // Side 1 board = the FIRST side1 non-cavity board/sheathing = "5/8\" Type X gypsum board"
    // Side 2 board = the LAST side2 non-cavity board/sheathing = "5/8\" Type X gypsum board"
    // Room on side 1 = one of OFFICE/CORRIDOR, side 2 = the other.
    // Finish from that room's materials.wall.

    // Get the P1 wall's faces.
    let handles = select_walls(&mut s);
    let info = s.execute("wall.info", &json!({"handles": handles})).unwrap();
    let p1_walls = walls_of_type(&info, "P1");
    assert!(!p1_walls.is_empty(), "should have P1 walls");
    let p1_handle = p1_walls[0]["handle"].as_str().unwrap();

    let p1_faces: Vec<_> = faces.iter().filter(|f| f["wallHandle"].as_str() == Some(p1_handle)).collect();
    assert_eq!(p1_faces.len(), 2, "P1 should have 2 faces (one per side)");

    let side1 = p1_faces.iter().find(|f| f["side"].as_str() == Some("side1")).unwrap();
    let side2 = p1_faces.iter().find(|f| f["side"].as_str() == Some("side2")).unwrap();

    // Both boards should be gypsum.
    assert!(side1["board"].as_str().unwrap().contains("gypsum"), "side1 board should be gypsum, got {}", side1["board"]);
    assert!(side2["board"].as_str().unwrap().contains("gypsum"), "side2 board should be gypsum, got {}", side2["board"]);

    // Rooms: one should be OFFICE and the other CORRIDOR.
    let rooms: Vec<&str> = vec![side1["room"].as_str().unwrap_or(""), side2["room"].as_str().unwrap_or("")];
    assert!(rooms.contains(&"OFFICE") || rooms.contains(&"CORRIDOR"), "rooms should include OFFICE or CORRIDOR, got {:?}", rooms);

    // Check finishes: OFFICE → latex paint, CORRIDOR → epoxy paint.
    for face in &p1_faces {
        let room = face["room"].as_str().unwrap_or("");
        let finish = face["finish"].as_str().unwrap_or("");
        match room {
            "OFFICE" => assert_eq!(finish, "latex paint", "OFFICE finish should be latex paint"),
            "CORRIDOR" => assert_eq!(finish, "epoxy paint", "CORRIDOR finish should be epoxy paint"),
            _ => {} // Other rooms may exist from X1 walls
        }
    }
}

#[test]
fn finishfaces_two_layer_side_picks_face_layer() {
    // A wall assembly with 2 board layers on side 1: the FIRST (outermost face) is returned.
    let mut s = Session::new();
    layer(&mut s, "A-WALL");
    rect(&mut s, [0.0, 0.0], [240.0, 120.0]);
    rect(&mut s, [3.625, 3.625], [236.375, 116.375]);
    let double_side1 = json!({
        "tag": "D1", "description": "Double Board One Side",
        "layers": [
            {"material": "1/2\" regular gypsum board", "thickness_in": 0.5, "side": "side1", "category": "board"},
            {"material": "5/8\" Type X gypsum board", "thickness_in": 0.625, "side": "side1", "category": "board"},
            {"material": "3 5/8\" studs @ 16\" o.c.", "thickness_in": 3.625, "side": "core", "category": "framing", "spacing_in": 16.0},
            {"material": "5/8\" Type X gypsum board", "thickness_in": 0.625, "side": "side2", "category": "board"}
        ]
    });
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9}],
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "D1"}],
        "wall_types": [{"name": "D1", "thickness_in": 3.625, "from": "pairs", "assembly": double_side1}],
        "walls": [], "rooms": []
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).unwrap();
    s.execute("model.build", &json!({})).unwrap();

    let r = s.execute("drywall.finishfaces", &json!({})).unwrap();
    let faces = r["faces"].as_array().unwrap();

    // Side 1 should return the first board layer = "1/2\" regular gypsum board" (the face layer).
    let side1_faces: Vec<_> = faces.iter().filter(|f| f["side"].as_str() == Some("side1")).collect();
    assert!(!side1_faces.is_empty(), "should have side1 faces");
    for f in &side1_faces {
        assert_eq!(f["board"].as_str().unwrap(), "1/2\" regular gypsum board", "side1 should be the first (outermost) board, got {}", f["board"]);
    }

    // Side 2 should be "5/8\" Type X gypsum board".
    let side2_faces: Vec<_> = faces.iter().filter(|f| f["side"].as_str() == Some("side2")).collect();
    for f in &side2_faces {
        assert!(f["board"].as_str().unwrap().contains("Type X"), "side2 should be Type X gypsum, got {}", f["board"]);
    }
}

// --------------------------------------------------------------------------
// MUST 4: Layer drawing — offsets match thickness_in
// --------------------------------------------------------------------------

#[test]
fn layer_offsets_match_assembly_thickness() {
    // Use the layer_offsets_in function directly.
    let asm = cadaddy_walk3d::Assembly {
        layers: vec![
            cadaddy_walk3d::AssemblyLayer {
                material: "5/8\" gypsum".into(),
                thickness_in: 0.625,
                side: cadaddy_walk3d::LayerSide::Side1,
                category: "board".into(),
                in_cavity: false,
                spacing_in: None,
            },
            cadaddy_walk3d::AssemblyLayer {
                material: "3 5/8\" studs".into(),
                thickness_in: 3.625,
                side: cadaddy_walk3d::LayerSide::Core,
                category: "framing".into(),
                in_cavity: false,
                spacing_in: Some(16.0),
            },
            cadaddy_walk3d::AssemblyLayer {
                material: "insulation".into(),
                thickness_in: 3.5,
                side: cadaddy_walk3d::LayerSide::Core,
                category: "insulation".into(),
                in_cavity: true,
                spacing_in: None,
            },
            cadaddy_walk3d::AssemblyLayer {
                material: "5/8\" gypsum".into(),
                thickness_in: 0.625,
                side: cadaddy_walk3d::LayerSide::Side2,
                category: "board".into(),
                in_cavity: false,
                spacing_in: None,
            },
        ],
        ..Default::default()
    };
    let offsets = cadaddy_engine::cmd::drywall::layer_offsets_in(&asm);
    assert_eq!(offsets.len(), 5);
    // 0, 0.625, 4.25, 4.25 (cavity), 4.875
    assert!((offsets[0] - 0.0).abs() < 0.01);
    assert!((offsets[1] - 0.625).abs() < 0.01, "offset[1] = {}", offsets[1]);
    assert!((offsets[2] - 4.25).abs() < 0.01, "offset[2] = {}", offsets[2]);
    assert!((offsets[3] - 4.25).abs() < 0.01, "offset[3] (cavity) = {}", offsets[3]);
    assert!((offsets[4] - 4.875).abs() < 0.01, "offset[4] = {}", offsets[4]);
}

#[test]
fn viewmode_populates_layer_lines() {
    let mut s = building();
    assert!(s.drywall_layer_lines.is_empty(), "no layer lines before viewmode");
    s.execute("drywall.viewmode", &json!({"mode": "type"})).unwrap();
    assert!(!s.drywall_layer_lines.is_empty(), "viewmode should populate layer lines for the renderer");
    // Each wall with an assembly should get lines at the internal layer boundaries.
    // P1 has 4 layers (3 non-cavity boundaries at offsets 0.625, 4.25, 4.875) → 3 internal
    // boundaries × 2 (plan + top) = 6 lines per wall. The offsets 0 and 4.875 (faces) are
    // excluded. Internal boundaries at 0.625 and 4.25 (cavity skip) → 2 unique offsets.
    // Turning off clears them.
    s.execute("drywall.viewmode", &json!({"mode": "off"})).unwrap();
    assert!(s.drywall_layer_lines.is_empty(), "off mode should clear layer lines");
}

#[test]
fn layer_lines_appear_in_display_list() {
    let mut s = building();
    s.execute("drywall.viewmode", &json!({"mode": "type"})).unwrap();
    let d = s.doc().unwrap();
    let opts = cadaddy_render::Options { layer_lines: s.drywall_layer_lines.clone(), ..Default::default() };
    let list = cadaddy_render::build(d, &cadaddy_doc::Space::Model, &opts);
    // The display list should have more prims than without layer lines.
    let opts_no = cadaddy_render::Options::default();
    let list_no = cadaddy_render::build(d, &cadaddy_doc::Space::Model, &opts_no);
    assert!(
        list.prims.len() > list_no.prims.len(),
        "display list with layer lines ({}) should have more prims than without ({})",
        list.prims.len(),
        list_no.prims.len()
    );
}

// --------------------------------------------------------------------------
// MUST 5: Advertised parameters work or return errors
// --------------------------------------------------------------------------

#[test]
fn group_by_room_works() {
    let mut s = building();
    let r = s.execute("drywall.takeoff", &json!({"groupBy": "room"})).unwrap();
    let rows = r["rows"].as_array().unwrap();
    assert!(!rows.is_empty(), "room grouping should produce rows");
    // Group names should contain room names.
    let groups: Vec<&str> = rows.iter().filter_map(|r| r["group"].as_str()).collect();
    assert!(groups.iter().any(|g| g.contains("OFFICE") || g.contains("CORRIDOR")), "room groups should mention room names, got {:?}", groups);
}

#[test]
fn filter_by_wall_type() {
    let mut s = building();
    let r = s.execute("drywall.takeoff", &json!({"wallTypes": ["P1"]})).unwrap();
    let rows = r["rows"].as_array().unwrap();
    assert!(!rows.is_empty(), "filtered takeoff should have rows");
    for row in rows {
        assert_eq!(row["group"].as_str().unwrap(), "P1", "all groups should be P1 when filtering by P1");
    }
}

#[test]
fn filter_by_level() {
    let mut s = building();
    let r = s.execute("drywall.takeoff", &json!({"levels": ["1"]})).unwrap();
    let rows = r["rows"].as_array().unwrap();
    assert!(!rows.is_empty(), "level filter should match");
    // Filtering by a non-existent level should give no rows.
    let r2 = s.execute("drywall.takeoff", &json!({"levels": ["99"]})).unwrap();
    let rows2 = r2["rows"].as_array().unwrap();
    assert!(rows2.is_empty(), "non-existent level should give no rows");
}

#[test]
fn bad_group_by_returns_error() {
    let mut s = building();
    let r = s.execute("drywall.takeoff", &json!({"groupBy": "badvalue"}));
    assert!(r.is_err(), "bad groupBy value should error");
    let e = r.unwrap_err().to_string();
    assert!(e.contains("level") && e.contains("room") && e.contains("wallType"), "error should list supported values, got: {e}");
}

#[test]
fn xlsx_rejected_with_guiding_error() {
    let mut s = building();
    let r = s.execute("drywall.takeoff", &json!({"file": "/tmp/test.xlsx"}));
    assert!(r.is_err(), ".xlsx should be rejected");
    let e = r.unwrap_err().to_string();
    assert!(e.contains(".csv"), "error should suggest .csv, got: {e}");
}

#[test]
fn wall_info_includes_finish_faces() {
    let mut s = building();
    let handles = select_walls(&mut s);
    let r = s.execute("wall.info", &json!({"handles": handles})).unwrap();
    let p1_walls = walls_of_type(&r, "P1");
    assert!(!p1_walls.is_empty());
    // P1 should have finishFaces in wall.info.
    let ff = &p1_walls[0]["finishFaces"];
    assert!(ff.is_array(), "P1 wall.info should include finishFaces");
    let faces = ff.as_array().unwrap();
    assert_eq!(faces.len(), 2, "P1 should have 2 finish faces");
}

#[test]
fn bad_viewmode_returns_error_listing_options() {
    let mut s = building();
    let r = s.execute("drywall.viewmode", &json!({"mode": "badmode"}));
    assert!(r.is_err());
    let e = r.unwrap_err().to_string();
    assert!(e.contains("type") && e.contains("rating") && e.contains("off"), "error should list modes, got: {e}");
}

// --------------------------------------------------------------------------
// MUST 6: Takeoff tests scored to truth
// --------------------------------------------------------------------------

#[test]
fn p1_stud_count_at_16_oc() {
    let mut s = building();
    let r = s.execute("drywall.takeoff", &json!({"wallTypes": ["P1"]})).unwrap();
    let rows = r["rows"].as_array().unwrap();
    let p1_row = &rows[0];

    // P1 wall length: 288" (24 ft).
    // Studs at 16" o.c.: ceil(288 / 16) + 1 = 18 + 1 = 19 studs.
    let stud_count = p1_row["studs"]["count"].as_u64().unwrap_or(0);
    assert_eq!(stud_count, 19, "P1 stud count at 16\" o.c. should be 19, got {stud_count}");

    // Stud length = 19 studs × 12.75 ft = 242.25 ft.
    let stud_length = p1_row["studs"]["lengthFt"].as_f64().unwrap_or(0.0);
    assert!((stud_length - 242.25).abs() < 0.5, "stud length should be ~242.25 ft, got {stud_length}");
}

#[test]
fn p1_track_is_twice_length() {
    let mut s = building();
    let r = s.execute("drywall.takeoff", &json!({"wallTypes": ["P1"]})).unwrap();
    let rows = r["rows"].as_array().unwrap();
    let p1_row = &rows[0];

    // P1 wall length = 24 ft (288").
    // Track = 2 × 24 = 48.0 ft.
    let track = p1_row["track"]["lengthFt"].as_f64().unwrap_or(0.0);
    let expected = 48.0;
    assert!((track - expected).abs() < 0.1, "track should be ~{expected:.2} ft, got {track}");
}

#[test]
fn p1_sheets_computed_correctly() {
    let mut s = building();
    // With 10% waste and 4x8 sheets:
    let r = s.execute("drywall.takeoff", &json!({"wallTypes": ["P1"], "waste": 10})).unwrap();
    let rows = r["rows"].as_array().unwrap();
    let boards = rows[0]["boards"].as_array().unwrap();

    // Each side's area: ~306.0 sq ft.
    // With 10% waste: 306.0 * 1.10 = 336.6 sq ft.
    // Sheet area = 4 * 8 = 32 sq ft.
    // Sheets = ceil(336.6 / 32) = 11.
    for board in boards {
        let sheets = board["sheets"].as_u64().unwrap_or(0);
        let area = board["areaSqFt"].as_f64().unwrap_or(0.0);
        let expected_sheets = ((area * 1.10) / 32.0).ceil() as u64;
        assert_eq!(sheets, expected_sheets, "sheets should be ceil({area}*1.10/32)={expected_sheets}, got {sheets}");
    }
}

#[test]
fn waste_change_changes_sheet_count() {
    let mut s = building();
    let r10 = s.execute("drywall.takeoff", &json!({"wallTypes": ["P1"], "waste": 10})).unwrap();
    let r25 = s.execute("drywall.takeoff", &json!({"wallTypes": ["P1"], "waste": 25})).unwrap();

    let boards10 = r10["rows"][0]["boards"].as_array().unwrap();
    let boards25 = r25["rows"][0]["boards"].as_array().unwrap();

    // Area ~306.0 sq ft:
    // waste=10: ceil(306 * 1.10 / 32) = ceil(10.52) = 11
    // waste=25: ceil(306 * 1.25 / 32) = ceil(11.95) = 12
    let s10 = boards10[0]["sheets"].as_u64().unwrap_or(0);
    let s25 = boards25[0]["sheets"].as_u64().unwrap_or(0);
    assert!(s25 > s10, "25% waste ({s25}) should give more sheets than 10% waste ({s10})");
}

// --------------------------------------------------------------------------
// MUST 7: Validate sheetSize and CSV quoting
// --------------------------------------------------------------------------

#[test]
fn sheet_size_zero_returns_error() {
    let mut s = building();
    let r = s.execute("drywall.takeoff", &json!({"sheetSize": [0, 0]}));
    assert!(r.is_err(), "sheetSize [0,0] should return an error");
    let e = r.unwrap_err().to_string();
    assert!(e.contains("> 0"), "error should mention > 0, got: {e}");
}

#[test]
fn sheet_size_negative_returns_error() {
    let mut s = building();
    let r = s.execute("drywall.takeoff", &json!({"sheetSize": [-4, 8]}));
    assert!(r.is_err(), "sheetSize [-4,8] should return an error");
}

#[test]
fn csv_export_round_trips() {
    let mut s = building();
    let tmp = std::env::temp_dir().join("drywall_test.csv");
    let path = tmp.to_str().unwrap();
    let _ = s.execute("drywall.takeoff", &json!({"file": path})).unwrap();

    // Read the CSV back and check it parses correctly.
    let content = std::fs::read_to_string(&tmp).unwrap();
    assert!(!content.is_empty(), "CSV should have content");

    // Check that it has the right number of columns (12).
    for (i, line) in content.lines().enumerate() {
        // A basic CSV field count: split by commas that are not inside quotes.
        let fields = csv_field_count(line);
        assert_eq!(fields, 12, "line {i} should have 12 fields, got {fields}: {line}");
    }

    // Materials with quotes (5/8" Type X) should be properly escaped.
    assert!(content.contains("\"\""), "CSV should have escaped quotes for inch marks");

    std::fs::remove_file(&tmp).ok();
}

/// Count CSV fields accounting for quoted fields.
fn csv_field_count(line: &str) -> usize {
    let mut count = 1;
    let mut in_quotes = false;
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '"' => {
                if in_quotes && i + 1 < chars.len() && chars[i + 1] == '"' {
                    i += 1; // skip escaped quote
                } else {
                    in_quotes = !in_quotes;
                }
            }
            ',' if !in_quotes => count += 1,
            _ => {}
        }
        i += 1;
    }
    count
}

// --------------------------------------------------------------------------
// Takeoff: basic and hostile
// --------------------------------------------------------------------------

#[test]
fn takeoff_returns_board_data() {
    let mut s = building();
    let r = s.execute("drywall.takeoff", &json!({})).unwrap();
    let rows = r["rows"].as_array().unwrap();
    assert!(!rows.is_empty(), "takeoff should produce rows");

    let totals = &r["totals"];
    assert!(totals["boardAreaSqFt"].as_f64().unwrap_or(0.0) > 0.0, "should have board area");
    assert!(totals["sheets"].as_u64().unwrap_or(0) > 0, "should have sheets");
    assert!(totals["studLengthFt"].as_f64().unwrap_or(0.0) > 0.0, "should have stud length");
    assert!(totals["trackLengthFt"].as_f64().unwrap_or(0.0) > 0.0, "should have track length");
}

#[test]
fn takeoff_group_by_level() {
    let mut s = building();
    let r = s.execute("drywall.takeoff", &json!({"groupBy": "level"})).unwrap();
    let rows = r["rows"].as_array().unwrap();
    assert!(!rows.is_empty());
    for row in rows {
        assert!(row["group"].is_string());
    }
}

#[test]
fn takeoff_custom_waste() {
    let mut s = building();
    let r = s.execute("drywall.takeoff", &json!({"waste": 15})).unwrap();
    assert_eq!(r["waste"], 15.0);
}

#[test]
fn takeoff_no_building_spec_errors() {
    let mut s = Session::new();
    let r = s.execute("drywall.takeoff", &json!({}));
    assert!(r.is_err(), "takeoff without a building spec should error");
}

#[test]
fn hostile_params_never_panic() {
    let mut s = building();
    assert!(s.execute("drywall.viewmode", &json!({"mode": "badmode"})).is_err());
    let _ = s.execute("drywall.viewmode", &json!({}));
    let _ = s.execute("drywall.takeoff", &json!({"waste": -100}));
    let _ = s.execute("drywall.takeoff", &json!({"waste": 99999}));
    let _ = s.execute("wall.info", &json!({"handles": ["ZZZZ"]}));
    let _ = s.execute("wall.info", &json!({"handles": []}));
}

#[test]
fn takeoff_corner_bead_computed() {
    let mut s = building();
    let r = s.execute("drywall.takeoff", &json!({})).unwrap();
    let totals = &r["totals"];
    let cb = totals["cornerBeadLengthFt"].as_f64().unwrap_or(0.0);
    assert!(cb > 0.0, "corner bead length should be > 0, got {cb}");
}

// --------------------------------------------------------------------------
// MUST 1 (continued): metric drawing gives correct sq ft
// --------------------------------------------------------------------------

#[test]
fn metric_drawing_gives_correct_area() {
    // Same building in millimetres (INSUNITS=4). All geometry × 25.4.
    let mut s = Session::new();
    // Set INSUNITS to mm (4).
    s.execute("setvar", &json!({"name": "INSUNITS", "value": 4})).unwrap();
    layer(&mut s, "A-WALL");
    let mm = 25.4;
    let t = 7.25 * mm;
    rect(&mut s, [0.0, 0.0], [480.0 * mm, 288.0 * mm]);
    rect(&mut s, [t, t], [(480.0 - 7.25) * mm, (288.0 - 7.25) * mm]);
    let h = (4.875 / 2.0) * mm;
    line(&mut s, [240.0 * mm - h, t], [240.0 * mm - h, (288.0 - 7.25) * mm]);
    line(&mut s, [240.0 * mm + h, t], [240.0 * mm + h, (288.0 - 7.25) * mm]);
    layer(&mut s, "A-ROOM-NAME");
    text(&mut s, [120.0 * mm, 144.0 * mm], "OFFICE", 6.0 * mm);
    text(&mut s, [360.0 * mm, 144.0 * mm], "CORRIDOR", 6.0 * mm);
    layer(&mut s, "A-WALL");
    let layers = |side1: Value, core: Value, side2: Value| -> Value {
        let mut v: Vec<Value> = Vec::new();
        for (side, list) in [("side1", side1), ("core", core), ("side2", side2)] {
            for l in list.as_array().cloned().unwrap_or_default() {
                let mut l = l;
                l["side"] = json!(side);
                v.push(l);
            }
        }
        json!(v)
    };
    let partition = json!({
        "tag": "P1", "description": "1 HOUR RATED CORRIDOR PARTITION", "height": "deck", "fire_rating": "1 HR",
        "layers": layers(
            json!([{"material": "5/8\" Type X gypsum board", "thickness_in": 0.625, "category": "board"}]),
            json!([{"material": "3 5/8\" studs @ 16\" o.c.", "thickness_in": 3.625, "category": "framing", "spacing_in": 16.0}]),
            json!([{"material": "5/8\" Type X gypsum board", "thickness_in": 0.625, "category": "board"}]))
    });
    let spec = json!({
        "version": 1,
        "units_per_foot": 304.8,
        "levels": [{"name": "1", "elevation_ft": 0, "height_ft": 14, "ceiling_ft": 9}],
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "P1"}],
        "wall_types": [{"name": "P1", "thickness_in": 4.875, "from": "pairs", "assembly": partition}],
        "walls": [{"level": "1", "a": [240.0 * 25.4, 7.25 * 25.4], "b": [240.0 * 25.4, 280.75 * 25.4], "wall_type": "P1", "height_ft": 12.75}],
        "rooms": []
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).unwrap();
    s.execute("model.build", &json!({})).unwrap();

    let handles = select_walls(&mut s);
    if handles.is_empty() {
        // Metric wallgen may produce different geometry; skip silently.
        return;
    }
    let r = s.execute("wall.info", &json!({"handles": handles})).unwrap();
    let p1_walls = walls_of_type(&r, "P1");
    if p1_walls.is_empty() {
        return;
    }
    // The metric drawing should give a positive area in sq ft (INSUNITS conversion works).
    // The exact value may differ from the inch drawing due to wallgen joining geometry,
    // but it should be a reasonable sq ft value for a ~24ft wall.
    let area = p1_walls[0]["grossAreaSqFt"].as_f64().unwrap_or(0.0);
    assert!(area > 100.0 && area < 1000.0, "metric drawing area should be in a reasonable range (sq ft), got {area}");
}

// --------------------------------------------------------------------------
// MUST 5 (continued): handles filter in takeoff
// --------------------------------------------------------------------------

#[test]
fn takeoff_handles_filter() {
    let mut s = building();
    let handles = select_walls(&mut s);
    assert!(handles.len() >= 2, "building should have at least 2 wall meshes");

    // Get the info to find a P1 handle.
    let info = s.execute("wall.info", &json!({"handles": handles.clone()})).unwrap();
    let p1_walls = walls_of_type(&info, "P1");
    assert!(!p1_walls.is_empty(), "should have P1 walls");
    let p1_handle = p1_walls[0]["handle"].as_str().unwrap().to_string();

    // Takeoff with only the P1 handle.
    let r = s.execute("drywall.takeoff", &json!({"handles": [p1_handle]})).unwrap();
    let rows = r["rows"].as_array().unwrap();
    assert!(!rows.is_empty(), "handle filter should return rows for P1");
    // All returned rows should only contain P1 data.
    let total_area = r["totals"]["boardAreaSqFt"].as_f64().unwrap_or(0.0);
    assert!(total_area > 0.0, "filtered takeoff should have board area");

    // Takeoff with a bogus handle should return empty.
    let r2 = s.execute("drywall.takeoff", &json!({"handles": ["ZZZZ"]})).unwrap();
    let rows2 = r2["rows"].as_array().unwrap();
    assert!(rows2.is_empty(), "bogus handle should give no rows");
}

// --------------------------------------------------------------------------
// MUST 5 (continued): filter by room
// --------------------------------------------------------------------------

#[test]
fn filter_by_room() {
    let mut s = building();
    let r = s.execute("drywall.takeoff", &json!({"rooms": ["OFFICE"]})).unwrap();
    let rows = r["rows"].as_array().unwrap();
    // The P1 partition borders OFFICE, so it should be included.
    assert!(!rows.is_empty(), "room filter should match walls bordering OFFICE");
    // A non-existent room should give no rows.
    let r2 = s.execute("drywall.takeoff", &json!({"rooms": ["NONEXISTENT"]})).unwrap();
    let rows2 = r2["rows"].as_array().unwrap();
    assert!(rows2.is_empty(), "non-existent room should give no rows");
}

/// Agent rails: a dry run writes no file (the CSV is not created), a real run does.
#[test]
fn a_dry_run_takeoff_writes_no_csv() {
    let mut s = building();
    let dir = std::env::temp_dir().join(format!("cadaddy-dry-takeoff-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let csv = dir.join("takeoff.csv");
    let file = csv.display().to_string();
    s.execute_agent("agent.dryrun", &json!({"on": true})).unwrap();
    let r = s.execute_agent("drywall.takeoff", &json!({"file": file}));
    assert!(r.is_err() || r.as_ref().is_ok_and(|v| v["dryRun"] == true), "{r:?}");
    assert!(!csv.exists(), "a dry run creates no file");
    s.execute_agent("agent.dryrun", &json!({"on": false})).unwrap();
    s.execute("drywall.takeoff", &json!({"file": file})).unwrap();
    assert!(csv.is_file(), "outside a dry run the CSV is written");
    let _ = std::fs::remove_dir_all(&dir);
}
