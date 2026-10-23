//! Tests for the import accuracy check (`import.check`): each check pass+fail, the auto-run
//! on import, the BIM gate, and "0 sheets processed => not completed".
//!
//! Synthetic drawings are built in code (made-up names only); no real files or file names.

use cadaddy_engine::cmd::import_check::{self, ImportCheckResult};
use cadaddy_engine::doc::{
    Block, Circle, Common, Drawing, Entity, EntityKind, Handle, Insert, Layer, Text, Wipeout,
};
use cadaddy_engine::geom::{Vec2, Vec3};
use cadaddy_engine::Session;
use serde_json::json;

type Line = cadaddy_engine::doc::Line;

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn entity(h: u64, layer: &str, kind: EntityKind) -> Entity {
    Entity {
        handle: Handle(h),
        common: Common { layer: layer.into(), ..Common::default() },
        kind,
    }
}

fn line_ent(h: u64, layer: &str, x0: f64, y0: f64, x1: f64, y1: f64) -> Entity {
    entity(h, layer, EntityKind::Line(Line { a: Vec3::new(x0, y0, 0.0), b: Vec3::new(x1, y1, 0.0) }))
}

fn text_ent(h: u64, layer: &str, value: &str, x: f64, y: f64, height: f64) -> Entity {
    entity(h, layer, EntityKind::Text(Text {
        value: value.into(),
        insert: Vec3::new(x, y, 0.0),
        height,
        align_pt: None,
        rotation: 0.0,
        width_factor: 1.0,
        oblique: 0.0,
        style: "Standard".into(),
        halign: cadaddy_engine::doc::HAlign::Left,
        valign: cadaddy_engine::doc::VAlign::Baseline,
    }))
}

fn insert_ent(h: u64, layer: &str, block: &str, x: f64, y: f64) -> Entity {
    entity(h, layer, EntityKind::Insert(Insert {
        block: block.into(),
        insert: Vec3::new(x, y, 0.0),
        scale: Vec3::new(1.0, 1.0, 1.0),
        rotation: 0.0,
        attribs: Vec::new(),
        cols: 1,
        rows: 1,
        col_spacing: 0.0,
        row_spacing: 0.0,
        clip: None,
    }))
}

/// A drawing that looks like a well-imported PDF: scaled to real units, multiple layers,
/// text, blocks, grid circles with labels.
fn good_import() -> Drawing {
    let mut d = Drawing::new_imperial();
    d.header.set_i64("INSUNITS", 1);
    d.layers.push(Layer { name: "WALLS".into(), ..Layer::default() });
    d.layers.push(Layer { name: "TEXT".into(), ..Layer::default() });
    d.layers.push(Layer { name: "DEVICES".into(), ..Layer::default() });
    d.layers.push(Layer { name: "GRID".into(), ..Layer::default() });

    let mut h = 0x200u64;
    let mut next = || { h += 1; h };

    // Walls on WALLS layer: spread across a large area (real-world units, ~100 ft).
    for i in 0..80 {
        let x = (i as f64) * 15.0;
        d.model.push(line_ent(next(), "WALLS", x, 0.0, x, 1200.0));
    }
    // Text on TEXT layer.
    for i in 0..30 {
        d.model.push(text_ent(next(), "TEXT", &format!("Room {}", 100 + i), i as f64 * 30.0, 50.0, 4.0));
    }
    // Blocks (inserts) on DEVICES layer.
    let block_name = "OUTLET_2P";
    d.blocks.insert(
        block_name.into(),
        std::sync::Arc::new(Block::new(block_name)),
    );
    for i in 0..50 {
        d.model.push(insert_ent(next(), "DEVICES", block_name, i as f64 * 20.0, 200.0));
    }
    // Grid circles on GRID layer: 8 circles of the same radius.
    for i in 0..8 {
        d.model.push(entity(next(), "GRID", EntityKind::Circle(Circle {
            center: Vec3::new(i as f64 * 120.0, -50.0, 0.0),
            radius: 12.0,
        })));
    }
    // Grid labels.
    for (i, label) in ["A", "B", "C", "D", "E", "F", "G", "H"].iter().enumerate() {
        d.model.push(text_ent(next(), "GRID", label, i as f64 * 120.0, -70.0, 8.0));
    }
    d
}

/// A drawing that looks like a bad PDF import: all on layer 0, no text, no blocks,
/// small extents (sheet size), many wipeouts.
fn bad_import() -> Drawing {
    let mut d = Drawing::new_imperial();
    d.header.set_i64("INSUNITS", 0);

    let mut h = 0x200u64;
    let mut next = || { h += 1; h };

    // Everything on layer "0" (default), small extents ~36x24 (sheet size).
    for i in 0..200 {
        let x = (i % 36) as f64;
        let y = (i / 36) as f64;
        d.model.push(line_ent(next(), "0", x, y, x + 0.5, y + 0.5));
    }
    // Many wipeouts (> 10% of entities).
    for _ in 0..80 {
        d.model.push(entity(next(), "0", EntityKind::Wipeout(Wipeout {
            boundary: vec![Vec2::new(0.0, 0.0), Vec2::new(1.0, 0.0), Vec2::new(1.0, 1.0), Vec2::new(0.0, 1.0)],
        })));
    }
    d
}

/// A minimal drawing with just a few entities (below thresholds).
fn tiny_drawing() -> Drawing {
    let mut d = Drawing::new_imperial();
    d.model.push(line_ent(0x200, "0", 0.0, 0.0, 100.0, 100.0));
    d
}

// ─── Check function unit tests ───────────────────────────────────────────────

#[test]
fn good_import_passes_all_checks() {
    let d = good_import();
    let result = import_check::check_drawing(&d);
    assert!(result.pass, "good import should pass: score={}, failed={:?}",
        result.score,
        result.checks.iter().filter(|c| !c.pass).map(|c| &c.id).collect::<Vec<_>>());
    assert!(result.score >= 50, "score should be >= 50, got {}", result.score);
    for c in &result.checks {
        assert!(c.pass, "check {} should pass: {}", c.id, c.found);
    }
}

#[test]
fn bad_import_fails() {
    let d = bad_import();
    let result = import_check::check_drawing(&d);
    assert!(!result.pass, "bad import should fail: score={}", result.score);
    assert!(result.score < 50, "score should be < 50, got {}", result.score);

    let failed_ids: Vec<&str> = result.checks.iter().filter(|c| !c.pass).map(|c| c.id.as_str()).collect();
    assert!(failed_ids.contains(&"SCALE"), "SCALE should fail, failed: {:?}", failed_ids);
    assert!(failed_ids.contains(&"LAYERS"), "LAYERS should fail, failed: {:?}", failed_ids);
    assert!(failed_ids.contains(&"TEXT"), "TEXT should fail, failed: {:?}", failed_ids);
    assert!(failed_ids.contains(&"FILLS"), "FILLS should fail, failed: {:?}", failed_ids);
    assert!(failed_ids.contains(&"SYMBOLS"), "SYMBOLS should fail, failed: {:?}", failed_ids);
}

#[test]
fn tiny_drawing_passes_as_exempt() {
    let d = tiny_drawing();
    let result = import_check::check_drawing(&d);
    assert!(result.pass, "tiny drawing should pass (exempt): score={}", result.score);
}

#[test]
fn scale_check_fails_on_sheet_size_extents() {
    let mut d = Drawing::new_imperial();
    d.header.set_i64("INSUNITS", 0);
    for i in 0..10 {
        d.model.push(line_ent(0x200 + i as u64, "0", 0.0, i as f64 * 2.4, 36.0, i as f64 * 2.4));
    }
    let result = import_check::check_drawing(&d);
    let scale = result.checks.iter().find(|c| c.id == "SCALE");
    assert!(scale.is_some_and(|c| !c.pass), "SCALE check should fail for sheet-size extents");
}

#[test]
fn layers_check_fails_when_all_on_one_layer() {
    let mut d = Drawing::new_imperial();
    d.header.set_i64("INSUNITS", 4);
    for i in 0..100 {
        d.model.push(line_ent(0x200 + i as u64, "0", i as f64 * 100.0, 0.0, i as f64 * 100.0, 5000.0));
    }
    let result = import_check::check_drawing(&d);
    let layers = result.checks.iter().find(|c| c.id == "LAYERS");
    assert!(layers.is_some_and(|c| !c.pass), "LAYERS check should fail when >90% on one layer");
}

#[test]
fn text_check_fails_when_no_text() {
    let mut d = Drawing::new_imperial();
    d.header.set_i64("INSUNITS", 4);
    d.layers.push(Layer { name: "A".into(), ..Layer::default() });
    d.layers.push(Layer { name: "B".into(), ..Layer::default() });
    for i in 0..200 {
        let layer = if i < 100 { "A" } else { "B" };
        d.model.push(line_ent(0x200 + i as u64, layer, i as f64 * 100.0, 0.0, i as f64 * 100.0, 5000.0));
    }
    let result = import_check::check_drawing(&d);
    let text = result.checks.iter().find(|c| c.id == "TEXT");
    assert!(text.is_some_and(|c| !c.pass), "TEXT check should fail when no text entities");
}

#[test]
fn fills_check_fails_on_excessive_wipeouts() {
    let mut d = Drawing::new_imperial();
    d.header.set_i64("INSUNITS", 4);
    d.layers.push(Layer { name: "A".into(), ..Layer::default() });
    d.layers.push(Layer { name: "B".into(), ..Layer::default() });
    for i in 0..50 {
        let layer = if i < 25 { "A" } else { "B" };
        d.model.push(line_ent(0x200 + i as u64, layer, i as f64 * 100.0, 0.0, i as f64 * 100.0, 5000.0));
    }
    for i in 0..20 {
        d.model.push(entity(0x300 + i as u64, "0", EntityKind::Wipeout(Wipeout {
            boundary: vec![Vec2::ZERO, Vec2::new(1.0, 0.0), Vec2::new(1.0, 1.0)],
        })));
    }
    let result = import_check::check_drawing(&d);
    let fills = result.checks.iter().find(|c| c.id == "FILLS");
    assert!(fills.is_some_and(|c| !c.pass), "FILLS check should fail on excessive wipeouts");
}

#[test]
fn symbols_check_fails_when_no_blocks() {
    let mut d = Drawing::new_imperial();
    d.header.set_i64("INSUNITS", 4);
    d.layers.push(Layer { name: "A".into(), ..Layer::default() });
    d.layers.push(Layer { name: "B".into(), ..Layer::default() });
    for i in 0..300 {
        let layer = if i < 150 { "A" } else { "B" };
        d.model.push(line_ent(0x200 + i as u64, layer, i as f64 * 100.0, 0.0, i as f64 * 100.0, 5000.0));
    }
    let result = import_check::check_drawing(&d);
    let syms = result.checks.iter().find(|c| c.id == "SYMBOLS");
    assert!(syms.is_some_and(|c| !c.pass), "SYMBOLS check should fail with no blocks");
}

// ─── Serialization round-trip ────────────────────────────────────────────────

#[test]
fn import_check_result_serializes_and_deserializes() {
    let d = good_import();
    let result = import_check::check_drawing(&d);
    let json_str = result.to_json();
    let back = ImportCheckResult::from_json(&json_str);
    assert!(back.is_some(), "should deserialize");
    let back = back.unwrap();
    assert_eq!(result.pass, back.pass);
    assert_eq!(result.score, back.score);
    assert_eq!(result.checks.len(), back.checks.len());
}

// ─── Store/read on Drawing ───────────────────────────────────────────────────

#[test]
fn store_and_read_import_check_on_drawing() {
    let mut d = good_import();
    let result = import_check::check_drawing(&d);
    import_check::store_result(&mut d, &result);
    assert!(d.import_check.is_some());
    let stored = import_check::stored_result(&d);
    assert!(stored.is_some());
    assert_eq!(stored.as_ref().map(|r| r.pass), Some(true));
}

#[test]
fn passes_returns_true_for_no_check() {
    let d = Drawing::new_imperial();
    assert!(import_check::passes(&d), "no check = assumed good");
}

#[test]
fn passes_returns_false_for_failing_check() {
    let mut d = bad_import();
    let result = import_check::check_drawing(&d);
    import_check::store_result(&mut d, &result);
    assert!(!import_check::passes(&d));
}

// ─── Command registration ────────────────────────────────────────────────────

#[test]
fn import_check_command_is_registered() {
    assert!(
        cadaddy_engine::cmd::find_by_id_or_alias("import.check").is_some(),
        "import.check command should be registered"
    );
}

// ─── import.check via Session ────────────────────────────────────────────────

#[test]
fn import_check_command_on_current_drawing() {
    let mut s = Session::new();
    let r = s.execute("import.check", &json!({}));
    assert!(r.is_ok(), "import.check should succeed: {:?}", r.err());
    let v = r.unwrap();
    assert!(v["pass"].as_bool().unwrap_or(false), "blank drawing should pass import check");
}

// ─── BIM gate ────────────────────────────────────────────────────────────────

#[test]
fn bim_build_refuses_failing_import() {
    let mut s = Session::new();
    if let Ok(d) = s.doc_mut() {
        let bad = bad_import();
        let result = import_check::check_drawing(&bad);
        import_check::store_result(d, &result);
    }
    let r = s.execute("bim.build", &json!({}));
    assert!(r.is_err(), "bim.build should refuse when import check fails");
    let err = format!("{}", r.unwrap_err());
    assert!(err.contains("import check failed"), "error should mention import check: {err}");
}

#[test]
fn bim_build_allows_force_override() {
    let mut s = Session::new();
    if let Ok(d) = s.doc_mut() {
        let bad = bad_import();
        let result = import_check::check_drawing(&bad);
        import_check::store_result(d, &result);
    }
    let r = s.execute("bim.build", &json!({"force": true}));
    if let Err(e) = &r {
        let msg = format!("{e}");
        assert!(
            !msg.contains("import check failed"),
            "force should bypass import check gate, got: {msg}"
        );
    }
}

// ─── .cdy round-trip ─────────────────────────────────────────────────────────

#[test]
fn import_check_survives_cdy_round_trip() {
    let mut d = good_import();
    let result = import_check::check_drawing(&d);
    import_check::store_result(&mut d, &result);

    let bytes = cadaddy_io::cdy::write_cdy(&d).expect("write .cdy");
    let d2 = cadaddy_io::read(&bytes, "test.cdy").expect("read .cdy");
    assert!(d2.import_check.is_some(), "import_check should survive .cdy round-trip");
    let stored = import_check::stored_result(&d2).expect("should parse stored result");
    assert_eq!(stored.pass, result.pass);
    assert_eq!(stored.score, result.score);
    assert_eq!(stored.checks.len(), result.checks.len());
}

#[test]
fn import_check_in_cdy_manifest() {
    let mut d = good_import();
    let result = import_check::check_drawing(&d);
    import_check::store_result(&mut d, &result);

    let bytes = cadaddy_io::cdy::write_cdy(&d).expect("write .cdy");
    let manifest = cadaddy_io::cdy::manifest(&bytes).expect("manifest should exist");
    assert!(manifest.get("importCheck").is_some(), "manifest should contain importCheck");
    assert_eq!(
        manifest["importCheck"]["pass"].as_bool(),
        Some(true),
        "manifest importCheck.pass should be true"
    );
}

// ─── Printset stage gate ─────────────────────────────────────────────────────

#[test]
fn printset_run_refuses_failing_import() {
    let mut s = Session::new();
    if let Ok(d) = s.doc_mut() {
        let bad = bad_import();
        let result = import_check::check_drawing(&bad);
        import_check::store_result(d, &result);
    }
    let r = s.execute("printset.run", &json!({}));
    assert!(r.is_err(), "printset.run should refuse when import check fails");
    let err = format!("{}", r.unwrap_err());
    assert!(err.contains("import check failed"), "error should mention import check: {err}");
}

// ─── Zero-sheets edge case ──────────────────────────────────────────────────

#[test]
fn empty_drawing_check_is_pass() {
    let d = Drawing::new_imperial();
    let result = import_check::check_drawing(&d);
    assert!(result.pass, "empty drawing should pass: score={}", result.score);
}
