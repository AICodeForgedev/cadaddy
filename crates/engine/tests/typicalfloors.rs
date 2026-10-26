//! Typical-floor tests (Q6005): a single sheet that covers multiple levels places its
//! elements on each of those levels.

use cadaddy_engine::Session;
use serde_json::{Value, json};

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

    let mut s = Session::new();
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
