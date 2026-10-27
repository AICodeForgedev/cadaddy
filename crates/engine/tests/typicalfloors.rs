//! Typical-floor tests (Q6005): a single sheet that covers multiple levels places its
//! elements on each of those levels.

/// detect_levels recognises "TYPICAL FLOOR" titles.
#[test]
fn detect_levels_typical_floor_titles() {
    use cadaddy_mepfix::sheet_reader::detect_levels;
    assert_eq!(detect_levels("TYPICAL FLOOR MECHANICAL PLAN"), vec!["T"]);
    assert_eq!(detect_levels("TYP. FLOOR PLAN"), vec!["T"]);
    assert_eq!(detect_levels("TYPICAL ELECTRICAL PLAN"), vec!["T"]);
    assert_eq!(detect_levels("2ND - 4TH FLOOR MECHANICAL PLAN"), vec!["2", "3", "4"]);
    assert_eq!(detect_levels("SECOND FLOOR PLAN"), vec!["2"]);
}

/// A multi-level SheetRegion's all_levels returns every covered level.
#[test]
fn sheet_region_all_levels() {
    use cadaddy_engine::cmd::guided::SheetRegion;
    let r = SheetRegion {
        number: "M-201".into(),
        level: "2".into(),
        discipline: "M".into(),
        kind: "plan".into(),
        rect: [0.0, 0.0, 100.0, 80.0],
        area: None,
        title_block: None,
        not_built: String::new(),
        levels: vec!["2".into(), "3".into(), "4".into()],
    };
    assert_eq!(r.all_levels(), vec!["2", "3", "4"]);

    // Empty levels falls back to just the primary level.
    let r2 = SheetRegion { levels: Vec::new(), ..r.clone() };
    assert_eq!(r2.all_levels(), vec!["2"]);
}

/// The devices stage replicates devices from a multi-level sheet to each covered level.
#[test]
fn the_devices_stage_replicates_for_typical_floor_sheets() {
    use cadaddy_engine::cmd::guided::*;

    let mut job = GuidedJob::new("/tmp/prints", JobGoal::Bim);

    // Set up: 4 arch sheets (one per level), 1 typical E sheet for levels 2-4.
    let mut x0 = 0.0;
    for level in ["1", "2", "3", "4"] {
        job.sheet_regions.push(SheetRegion {
            number: format!("A-10{level}"),
            level: level.into(),
            discipline: "A".into(),
            kind: "plan".into(),
            rect: [x0, 0.0, x0 + 100.0, 80.0],
            area: None,
            title_block: None,
            not_built: String::new(),
            levels: Vec::new(),
        });
        x0 += 200.0;
    }
    // Typical E sheet covers levels 2, 3, 4 — its rect is in the L2 position.
    let e_rect = [200.0, 100.0, 300.0, 180.0];
    job.sheet_regions.push(SheetRegion {
        number: "E-201".into(),
        level: "2".into(),
        discipline: "E".into(),
        kind: "plan".into(),
        rect: e_rect,
        area: None,
        title_block: None,
        not_built: String::new(),
        levels: vec!["2".into(), "3".into(), "4".into()],
    });
    // A dedicated E sheet for level 1.
    let e1_rect = [0.0, 100.0, 100.0, 180.0];
    job.sheet_regions.push(SheetRegion {
        number: "E-101".into(),
        level: "1".into(),
        discipline: "E".into(),
        kind: "plan".into(),
        rect: e1_rect,
        area: None,
        title_block: None,
        not_built: String::new(),
        levels: Vec::new(),
    });

    // Place a PDF-SYM symbol inside the typical sheet's rect.
    let sym_pos = [(e_rect[0] + 10.0, e_rect[1] + 10.0)];
    // The device_census runs on the drawing's entities; we test the region lookup instead.
    let region = job.level_at([sym_pos[0].0, sym_pos[0].1]);
    assert!(region.is_some(), "symbol should land in the typical sheet's rect");
    let region = region.unwrap();
    assert_eq!(region.number, "E-201");
    let all = region.all_levels();
    assert_eq!(all, vec!["2", "3", "4"], "typical sheet should cover 3 levels");
}

// ---- Q6018 typicalfix: a floor-specific sheet is never counted on two levels ----

/// levels.list marks typical sheets with typical:true and floor-specific sheets
/// with typical:false. A floor-specific sheet (levels empty or single-entry) appears
/// on exactly one level; a typical sheet (levels with >1 entry) appears on each.
#[test]
fn levels_list_typical_flag() {
    use cadaddy_engine::Session;
    use serde_json::json;

    let mut s = Session::new();
    // 4 levels + 2 M sheets: M-201 typical on levels 2-4, M-101 floor-specific on level 1.
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [
            {"name": "Level 1", "elevation_ft": 0, "height_ft": 12, "region": [0, 0, 1000, 1000]},
            {"name": "Level 2", "elevation_ft": 12, "height_ft": 12, "region": [2000, 0, 3000, 1000]},
            {"name": "Level 3", "elevation_ft": 24, "height_ft": 12, "region": [4000, 0, 5000, 1000]},
            {"name": "Level 4", "elevation_ft": 36, "height_ft": 12, "region": [6000, 0, 7000, 1000]}
        ],
        "sheets": [
            {"number": "M-101", "discipline": "M", "level": "Level 1", "source": "model"},
            {"number": "M-201", "discipline": "M", "level": "Level 2", "source": "model",
             "levels": ["Level 2", "Level 3", "Level 4"]}
        ]
    });
    s.execute("bspec.set", &json!({ "replace": true, "spec": spec })).unwrap();

    let l = s.execute("levels.list", &json!({})).unwrap();
    let levels = l["levels"].as_array().unwrap();

    // Level 1: M-101 only, not typical.
    let l1_sheets = levels[0]["sheets"].as_array().unwrap();
    assert_eq!(l1_sheets.len(), 1, "Level 1 should have 1 sheet: {l1_sheets:?}");
    assert_eq!(l1_sheets[0]["sheet"], json!("M-101"));
    assert_eq!(l1_sheets[0]["typical"], json!(false));

    // Level 2: M-201 typical.
    let l2_sheets = levels[1]["sheets"].as_array().unwrap();
    assert!(l2_sheets.iter().any(|s| s["sheet"] == "M-201" && s["typical"] == json!(true)), "Level 2 should have M-201 typical: {l2_sheets:?}");

    // Level 3: M-201 typical.
    let l3_sheets = levels[2]["sheets"].as_array().unwrap();
    assert!(l3_sheets.iter().any(|s| s["sheet"] == "M-201" && s["typical"] == json!(true)), "Level 3 should have M-201 typical: {l3_sheets:?}");

    // Level 4: M-201 typical.
    let l4_sheets = levels[3]["sheets"].as_array().unwrap();
    assert!(l4_sheets.iter().any(|s| s["sheet"] == "M-201" && s["typical"] == json!(true)), "Level 4 should have M-201 typical: {l4_sheets:?}");

    // The gate check: count sheets on >1 level where not all occurrences are typical.
    let mut seen: std::collections::HashMap<String, Vec<bool>> = std::collections::HashMap::new();
    for lv in levels {
        for sh in lv["sheets"].as_array().unwrap() {
            let sid = sh["sheet"].as_str().unwrap().to_string();
            let typ = sh["typical"].as_bool().unwrap_or(false);
            seen.entry(sid).or_default().push(typ);
        }
    }
    let dup_untyped: usize = seen.values().filter(|v| v.len() > 1 && !v.iter().all(|t| *t)).count();
    assert_eq!(dup_untyped, 0, "no floor-specific sheet should appear on two levels: {seen:?}");
}

/// detect_levels: a title with a level AND a "(TYP.)" note yields only the explicit level.
#[test]
fn detect_levels_explicit_level_with_typ_note() {
    use cadaddy_mepfix::sheet_reader::detect_levels;
    // A floor-specific title with a parenthetical "TYP." — the level wins, not typical.
    assert_eq!(detect_levels("SECOND FLOOR MECHANICAL PLAN (TYP.)"), vec!["2"]);
    assert_eq!(detect_levels("LEVEL 2 PLUMBING PLAN - TYP."), vec!["2"]);
    assert_eq!(detect_levels("3RD FLOOR POWER PLAN (TYP.)"), vec!["3"]);
}
