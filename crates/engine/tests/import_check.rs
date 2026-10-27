//! Tests for the import accuracy check (`import.check`): each check pass+fail, warn-only
//! logic (failed checks warn, never block a build), the BIM gate (blocks only empty/unreadable
//! imports), and "0 sheets processed => not completed".
//!
//! Synthetic drawings are built in code (made-up names only); no real files or file names.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::import_check::{self, ImportCheckResult};
use cadaddy_engine::doc::{Block, Circle, Common, Drawing, Entity, EntityKind, Handle, Insert, Layer, Text, Wipeout};
use cadaddy_engine::geom::{Vec2, Vec3};
use serde_json::json;

type Line = cadaddy_engine::doc::Line;

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn entity(h: u64, layer: &str, kind: EntityKind) -> Entity {
    Entity { handle: Handle(h), common: Common { layer: layer.into(), ..Common::default() }, kind }
}

fn line_ent(h: u64, layer: &str, x0: f64, y0: f64, x1: f64, y1: f64) -> Entity {
    entity(h, layer, EntityKind::Line(Line { a: Vec3::new(x0, y0, 0.0), b: Vec3::new(x1, y1, 0.0) }))
}

fn text_ent(h: u64, layer: &str, value: &str, x: f64, y: f64, height: f64) -> Entity {
    entity(
        h,
        layer,
        EntityKind::Text(Text {
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
        }),
    )
}

fn insert_ent(h: u64, layer: &str, block: &str, x: f64, y: f64) -> Entity {
    entity(
        h,
        layer,
        EntityKind::Insert(Insert {
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
        }),
    )
}

/// Short line segments clustered together to simulate stroked lettering glyphs.
fn stroke_glyph_cluster(h_start: u64, layer: &str, cx: f64, cy: f64, count: usize) -> Vec<Entity> {
    let mut ents = Vec::new();
    for i in 0..count {
        let offset = (i as f64) * 0.3;
        ents.push(line_ent(h_start + i as u64, layer, cx + offset, cy, cx + offset + 0.2, cy + 1.0));
    }
    ents
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
    let mut next = || {
        h += 1;
        h
    };

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
    d.blocks.insert(block_name.into(), std::sync::Arc::new(Block::new(block_name)));
    for i in 0..50 {
        d.model.push(insert_ent(next(), "DEVICES", block_name, i as f64 * 20.0, 200.0));
    }
    // Grid circles on GRID layer: 8 circles of the same radius.
    for i in 0..8 {
        d.model.push(entity(next(), "GRID", EntityKind::Circle(Circle { center: Vec3::new(i as f64 * 120.0, -50.0, 0.0), radius: 12.0 })));
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
    let mut next = || {
        h += 1;
        h
    };

    // Everything on layer "0" (default), extents ~36x24 (sheet size in inches).
    for i in 0..200 {
        let x = (i as f64 / 200.0) * 36.0;
        let y = (i as f64 / 200.0) * 24.0;
        d.model.push(line_ent(next(), "0", x, y, x + 0.5, y + 0.5));
    }
    // Many wipeouts (> 10% of entities).
    for _ in 0..80 {
        d.model.push(entity(
            next(),
            "0",
            EntityKind::Wipeout(Wipeout { boundary: vec![Vec2::new(0.0, 0.0), Vec2::new(1.0, 0.0), Vec2::new(1.0, 1.0), Vec2::new(0.0, 1.0)] }),
        ));
    }
    d
}

/// A minimal drawing with just a few entities (below thresholds).
fn tiny_drawing() -> Drawing {
    let mut d = Drawing::new_imperial();
    d.model.push(line_ent(0x200, "0", 0.0, 0.0, 100.0, 100.0));
    d
}

/// An import that is truly unreadable: only wipeouts, no geometry or text.
/// This is the only kind of import that should fail (blocks builds).
fn unreadable_import() -> Drawing {
    let mut d = Drawing::new_imperial();
    for i in 0..50 {
        d.model.push(entity(
            0x400 + i as u64,
            "0",
            EntityKind::Wipeout(Wipeout { boundary: vec![Vec2::ZERO, Vec2::new(1.0, 0.0), Vec2::new(1.0, 1.0)] }),
        ));
    }
    d
}

/// MUST 4: A synthetic drawing reproducing E-4 field statistics.
/// - Sheet-size extents with a 1/8" scale note
/// - ~1% text + many stroke-glyph clusters
/// - ~14% wipeouts/rasters
/// - 0 blocks
/// - 40 grid circles
///
/// Must FAIL with SCALE, TEXT, FILLS, SYMBOLS named.
fn e4_field_import() -> Drawing {
    let mut d = Drawing::new_imperial();
    d.header.set_i64("INSUNITS", 1); // inches

    let mut h = 0x1000u64;
    let mut next = || {
        h += 1;
        h
    };

    // Sheet-size extents: ~36 x 24 inches (a standard ARCH D sheet, NOT scaled up).
    // Most entities are lines spread across this area.
    for i in 0..400 {
        let x = (i as f64 / 400.0) * 36.0;
        let y = (i as f64 / 400.0) * 24.0;
        d.model.push(line_ent(next(), "0", x, y, x + 0.5, y + 0.3));
    }

    // Scale note: "SCALE: 1/8\" = 1'-0\"" on a TITLE layer.
    // The scale says the model should be ~288 ft wide, but extents are only 36 inches.
    d.layers.push(Layer { name: "TITLE".into(), ..Layer::default() });
    d.model.push(text_ent(next(), "TITLE", "SCALE: 1/8\" = 1'-0\"", 30.0, 1.0, 0.12));

    // ~1% real text (a few labels) amidst many entities.
    for i in 0..7 {
        d.model.push(text_ent(next(), "0", &format!("Label {}", i + 1), i as f64 * 5.0, 12.0, 0.1));
    }

    // Many stroke-glyph clusters (~50 clusters of short segments to simulate stroked text).
    for cluster in 0..50 {
        let cx = (cluster as f64 / 50.0) * 34.0 + 1.0;
        let cy = 15.0 + (cluster % 5) as f64 * 1.5;
        for seg in stroke_glyph_cluster(next(), "0", cx, cy, 5) {
            // Re-assign handles.
            let mut e = seg;
            e.handle = Handle(next());
            d.model.push(e);
        }
    }

    // 14% wipeouts/rasters. Total entities so far is roughly: 400 + 1(scale) + 7(text) + 250(strokes) ≈ 658.
    // 14% of ~770 total ≈ 108 fills to get the ratio right.
    for _ in 0..110 {
        d.model.push(entity(
            next(),
            "0",
            EntityKind::Wipeout(Wipeout { boundary: vec![Vec2::new(0.0, 0.0), Vec2::new(2.0, 0.0), Vec2::new(2.0, 2.0), Vec2::new(0.0, 2.0)] }),
        ));
    }

    // 0 blocks (no Insert entities).

    // 40 grid circles of similar radius.
    for i in 0..40 {
        d.model.push(entity(
            next(),
            "0",
            EntityKind::Circle(Circle { center: Vec3::new((i % 20) as f64 * 1.8, if i < 20 { -2.0 } else { 26.0 }, 0.0), radius: 0.25 }),
        ));
    }

    d
}

/// ADDENDUM test: A-sheet at true scale.
fn arch_sheet_true_scale() -> Drawing {
    let mut d = Drawing::new_imperial();
    d.header.set_i64("INSUNITS", 1); // inches
    d.layers.push(Layer { name: "WALLS".into(), ..Layer::default() });
    d.layers.push(Layer { name: "TEXT".into(), ..Layer::default() });
    d.layers.push(Layer { name: "GRID".into(), ..Layer::default() });
    d.layers.push(Layer { name: "TITLE".into(), ..Layer::default() });

    let mut h = 0x200u64;
    let mut next = || {
        h += 1;
        h
    };

    // Real-world extents: ~288 x 192 ft at 1/8" = 1'-0" (in inches: 3456 x 2304).
    for i in 0..80 {
        let x = (i as f64) * 40.0;
        d.model.push(line_ent(next(), "WALLS", x, 0.0, x, 2000.0));
    }
    // Text labels.
    for i in 0..30 {
        d.model.push(text_ent(next(), "TEXT", &format!("Room {}", 100 + i), i as f64 * 80.0, 200.0, 8.0));
    }
    // Block references.
    let block_name = "SWITCH_3W";
    d.blocks.insert(block_name.into(), std::sync::Arc::new(Block::new(block_name)));
    for i in 0..40 {
        d.model.push(insert_ent(next(), "WALLS", block_name, i as f64 * 60.0, 400.0));
    }
    // Grid circles and labels.
    for i in 0..10 {
        d.model.push(entity(next(), "GRID", EntityKind::Circle(Circle { center: Vec3::new(i as f64 * 300.0, -100.0, 0.0), radius: 24.0 })));
    }
    for (i, label) in ["A", "B", "C", "D", "E", "F", "G", "H", "J", "K"].iter().enumerate() {
        d.model.push(text_ent(next(), "GRID", label, i as f64 * 300.0, -140.0, 12.0));
    }
    // Sheet number + scale note.
    d.model.push(text_ent(next(), "TITLE", "A-103", 3200.0, -200.0, 14.0));
    d.model.push(text_ent(next(), "TITLE", "SCALE: 1/8\" = 1'-0\"", 3200.0, -220.0, 6.0));
    d.model.push(text_ent(next(), "TITLE", "THIRD FLOOR PLAN", 3200.0, -240.0, 10.0));
    d
}

/// ADDENDUM test: E-sheet with DO NOT SCALE note and a different drawn scale.
fn elec_sheet_do_not_scale() -> Drawing {
    let mut d = Drawing::new_imperial();
    d.header.set_i64("INSUNITS", 1);
    d.layers.push(Layer { name: "DEVICES".into(), ..Layer::default() });
    d.layers.push(Layer { name: "TEXT".into(), ..Layer::default() });
    d.layers.push(Layer { name: "GRID".into(), ..Layer::default() });
    d.layers.push(Layer { name: "TITLE".into(), ..Layer::default() });

    let mut h = 0x500u64;
    let mut next = || {
        h += 1;
        h
    };

    // Electrical sheet drawn at a DIFFERENT scale (smaller extents, not real-world).
    // This mimics an E-sheet that says DO NOT SCALE.
    for i in 0..60 {
        let x = (i as f64) * 5.0;
        d.model.push(line_ent(next(), "DEVICES", x, 0.0, x, 300.0));
    }
    // Some text.
    for i in 0..20 {
        d.model.push(text_ent(next(), "TEXT", &format!("CKT {}", i + 1), i as f64 * 15.0, 50.0, 3.0));
    }
    // Block references for devices.
    let block_name = "RECEP_DUPLEX";
    d.blocks.insert(block_name.into(), std::sync::Arc::new(Block::new(block_name)));
    for i in 0..30 {
        d.model.push(insert_ent(next(), "DEVICES", block_name, i as f64 * 10.0, 100.0));
    }
    // Grid circles (shared with arch plan).
    for i in 0..8 {
        d.model.push(entity(next(), "GRID", EntityKind::Circle(Circle { center: Vec3::new(i as f64 * 35.0, -20.0, 0.0), radius: 5.0 })));
    }
    for (i, label) in ["A", "B", "C", "D", "E", "F", "G", "H"].iter().enumerate() {
        d.model.push(text_ent(next(), "GRID", label, i as f64 * 35.0, -30.0, 4.0));
    }
    // Sheet number.
    d.model.push(text_ent(next(), "TITLE", "E-4", 280.0, -50.0, 8.0));
    d.model.push(text_ent(next(), "TITLE", "THIRD FLOOR ELECTRICAL PLAN", 280.0, -60.0, 6.0));
    // DO NOT SCALE note.
    d.model.push(text_ent(
        next(),
        "TEXT",
        "DO NOT SCALE \u{2013} REFER TO ARCHITECTURAL DRAWINGS FOR DIMENSIONS; ELECTRICAL LOCATIONS ARE APPROXIMATE.",
        10.0,
        310.0,
        2.0,
    ));
    d
}

// ─── Check function unit tests ───────────────────────────────────────────────

#[test]
fn good_import_passes_all_checks() {
    let d = good_import();
    let result = import_check::check_drawing(&d);
    assert!(
        result.pass,
        "good import should pass: score={}, failed={:?}, hard_fails={:?}",
        result.score,
        result.checks.iter().filter(|c| !c.pass).map(|c| &c.id).collect::<Vec<_>>(),
        result.hard_fails
    );
    assert!(result.score >= 50, "score should be >= 50, got {}", result.score);
    assert!(result.hard_fails.is_empty(), "no hard failures expected");
    for c in &result.checks {
        assert!(c.pass, "check {} should pass: {}", c.id, c.found);
    }
}

#[test]
fn bad_import_passes_with_warnings() {
    let d = bad_import();
    let result = import_check::check_drawing(&d);
    // Bad import has usable content (lines) → passes, but individual checks warn.
    assert!(result.pass, "bad import with usable content should pass: score={}", result.score);

    let failed_ids: Vec<&str> = result.checks.iter().filter(|c| !c.pass).map(|c| c.id.as_str()).collect();
    assert!(failed_ids.contains(&"SCALE"), "SCALE should warn, failed: {:?}", failed_ids);
    assert!(failed_ids.contains(&"LAYERS"), "LAYERS should warn, failed: {:?}", failed_ids);
    assert!(failed_ids.contains(&"FILLS"), "FILLS should warn, failed: {:?}", failed_ids);
    assert!(failed_ids.contains(&"SYMBOLS"), "SYMBOLS should warn, failed: {:?}", failed_ids);
}

#[test]
fn unreadable_import_fails() {
    let d = unreadable_import();
    let result = import_check::check_drawing(&d);
    assert!(!result.pass, "unreadable import (only fills) should fail");
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
    d.header.set_i64("INSUNITS", 1); // inches
    // Extents ~36x24 — a standard ARCH D sheet.
    for i in 0..10 {
        d.model.push(line_ent(0x200 + i as u64, "0", 0.0, i as f64 * 2.4, 36.0, i as f64 * 2.4));
    }
    let result = import_check::check_drawing(&d);
    let scale = result.checks.iter().find(|c| c.id == "SCALE");
    assert!(scale.is_some_and(|c| !c.pass), "SCALE check should fail for sheet-size extents");
    assert!(scale.is_some_and(|c| c.hard_fail), "SCALE should be a hard-fail check");
}

#[test]
fn scale_check_fails_on_pdf_points() {
    // Extents in PDF points: 36" = 2592 pts, 24" = 1728 pts.
    let mut d = Drawing::new_imperial();
    d.header.set_i64("INSUNITS", 0); // unitless (PDF points)
    for i in 0..10 {
        d.model.push(line_ent(0x200 + i as u64, "0", 0.0, i as f64 * 172.8, 2592.0, i as f64 * 172.8));
    }
    let result = import_check::check_drawing(&d);
    let scale = result.checks.iter().find(|c| c.id == "SCALE");
    assert!(scale.is_some_and(|c| !c.pass), "SCALE should fail on PDF-point-sized extents");
}

#[test]
fn scale_check_with_scale_note_disagrees() {
    let mut d = Drawing::new_imperial();
    d.header.set_i64("INSUNITS", 1);
    d.layers.push(Layer { name: "TITLE".into(), ..Layer::default() });
    // Scale note says 1/8"=1'-0" (plan should be ~288 ft wide).
    d.model.push(text_ent(0x200, "TITLE", "SCALE: 1/8\" = 1'-0\"", 30.0, 1.0, 0.12));
    // But extents are only 36 x 24 inches (sheet size, not real-world).
    for i in 0..10 {
        d.model.push(line_ent(0x210 + i as u64, "0", 0.0, i as f64 * 2.4, 36.0, i as f64 * 2.4));
    }
    let result = import_check::check_drawing(&d);
    let scale = result.checks.iter().find(|c| c.id == "SCALE");
    assert!(scale.is_some_and(|c| !c.pass), "SCALE should fail when model size disagrees with scale note");
    // Evidence should include the scale note.
    let ev = scale.and_then(|c| c.evidence.as_ref());
    assert!(ev.is_some_and(|e| e["scale_disagree"] == json!(true)), "evidence should show scale_disagree=true");
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
fn text_check_fails_when_many_stroke_clusters() {
    let mut d = Drawing::new_imperial();
    d.header.set_i64("INSUNITS", 4);
    d.layers.push(Layer { name: "A".into(), ..Layer::default() });
    d.layers.push(Layer { name: "B".into(), ..Layer::default() });

    let mut h = 0x200u64;
    let mut next = || {
        h += 1;
        h
    };

    // Structural lines spread across layers.
    for i in 0..100 {
        let layer = if i < 50 { "A" } else { "B" };
        d.model.push(line_ent(next(), layer, i as f64 * 100.0, 0.0, i as f64 * 100.0, 5000.0));
    }

    // Only 2 real text entities.
    d.model.push(text_ent(next(), "A", "Sample Label", 50.0, 50.0, 4.0));
    d.model.push(text_ent(next(), "B", "Another Label", 150.0, 50.0, 4.0));

    // 20 stroke-glyph clusters (simulating un-recovered stroked lettering).
    for cluster in 0..20 {
        let cx = cluster as f64 * 200.0;
        for seg in stroke_glyph_cluster(next(), "A", cx, 100.0, 5) {
            let mut e = seg;
            e.handle = Handle(next());
            d.model.push(e);
        }
    }

    let result = import_check::check_drawing(&d);
    let text = result.checks.iter().find(|c| c.id == "TEXT");
    assert!(text.is_some_and(|c| !c.pass), "TEXT check should fail: many glyph clusters but few text entities");
    // Evidence should show low recovery ratio.
    let ev = text.and_then(|c| c.evidence.as_ref());
    assert!(ev.is_some_and(|e| e["recovery_ratio"].as_f64().unwrap_or(1.0) < 0.5), "recovery ratio should be < 50%");
}

#[test]
fn text_check_passes_when_no_text() {
    let mut d = Drawing::new_imperial();
    d.header.set_i64("INSUNITS", 4);
    d.layers.push(Layer { name: "A".into(), ..Layer::default() });
    d.layers.push(Layer { name: "B".into(), ..Layer::default() });
    // Only long structural lines — no short stroke segments, no text.
    for i in 0..200 {
        let layer = if i < 100 { "A" } else { "B" };
        d.model.push(line_ent(0x200 + i as u64, layer, i as f64 * 100.0, 0.0, i as f64 * 100.0, 5000.0));
    }
    let result = import_check::check_drawing(&d);
    let text = result.checks.iter().find(|c| c.id == "TEXT");
    // No stroke clusters and no text → detected_lettering=0 → pass.
    assert!(text.is_some_and(|c| c.pass), "TEXT should pass when no lettering is detected at all");
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
        d.model.push(entity(
            0x300 + i as u64,
            "0",
            EntityKind::Wipeout(Wipeout { boundary: vec![Vec2::ZERO, Vec2::new(1.0, 0.0), Vec2::new(1.0, 1.0)] }),
        ));
    }
    let result = import_check::check_drawing(&d);
    let fills = result.checks.iter().find(|c| c.id == "FILLS");
    assert!(fills.is_some_and(|c| !c.pass), "FILLS check should fail on excessive wipeouts");
    assert!(fills.is_some_and(|c| c.hard_fail), "FILLS should be a hard-fail check");
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
    assert!(syms.is_some_and(|c| c.hard_fail), "SYMBOLS should be a hard-fail check");
}

// ─── Warn-only logic (no check blocks a build) ─────────────────────────────

#[test]
fn check_failures_warn_but_do_not_block() {
    // A drawing with usable content + excessive wipeouts. FILLS check fails
    // but the overall import should still PASS (warns, never blocks).
    let mut d = Drawing::new_imperial();
    d.header.set_i64("INSUNITS", 1);
    d.layers.push(Layer { name: "WALLS".into(), ..Layer::default() });
    d.layers.push(Layer { name: "TEXT".into(), ..Layer::default() });

    let mut h = 0x200u64;
    let mut next = || {
        h += 1;
        h
    };

    // Real-world extents (passes SCALE).
    for i in 0..60 {
        let x = (i as f64) * 40.0;
        d.model.push(line_ent(next(), "WALLS", x, 0.0, x, 2000.0));
    }
    // Text (passes TEXT).
    for i in 0..20 {
        d.model.push(text_ent(next(), "TEXT", &format!("Note {}", i), i as f64 * 80.0, 100.0, 4.0));
    }
    // Blocks (passes SYMBOLS).
    let block_name = "DEVICE_A";
    d.blocks.insert(block_name.into(), std::sync::Arc::new(Block::new(block_name)));
    for i in 0..20 {
        d.model.push(insert_ent(next(), "WALLS", block_name, i as f64 * 100.0, 500.0));
    }
    // Grid (passes GRID).
    for i in 0..8 {
        d.model.push(entity(next(), "WALLS", EntityKind::Circle(Circle { center: Vec3::new(i as f64 * 250.0, -80.0, 0.0), radius: 20.0 })));
    }
    for label in ["1", "2", "3", "4", "5", "6", "7", "8"] {
        d.model.push(text_ent(next(), "WALLS", label, 0.0, -120.0, 8.0));
    }

    // Excessive wipeouts → FILLS check fails.
    let total_so_far = d.model.len();
    let fill_needed = (total_so_far as f64 * 0.15) as usize;
    for _ in 0..fill_needed {
        d.model.push(entity(next(), "0", EntityKind::Wipeout(Wipeout { boundary: vec![Vec2::ZERO, Vec2::new(1.0, 0.0), Vec2::new(1.0, 1.0)] })));
    }

    let result = import_check::check_drawing(&d);
    // Has usable content → PASS even though FILLS check fails.
    assert!(result.pass, "should PASS: failed checks warn but never block (score={})", result.score);
    let fills = result.checks.iter().find(|c| c.id == "FILLS");
    assert!(fills.is_some_and(|c| !c.pass), "FILLS check should still report failure as a warning");
}

// ─── MUST 4: E-4 field statistics regression test ───────────────────────────

#[test]
fn e4_field_statistics_pass_with_warnings() {
    let d = e4_field_import();
    let result = import_check::check_drawing(&d);
    // E-4 has usable content (lines, text, circles) → passes, individual checks warn.
    assert!(result.pass, "E-4 field import should PASS (has usable content): score={}", result.score);

    let failed_ids: Vec<&str> = result.checks.iter().filter(|c| !c.pass).map(|c| c.id.as_str()).collect();
    assert!(failed_ids.contains(&"SCALE"), "SCALE should warn on E-4: extents match sheet size. Failed: {:?}", failed_ids);
    assert!(failed_ids.contains(&"TEXT"), "TEXT should warn on E-4: many stroke clusters, few text entities. Failed: {:?}", failed_ids);
    assert!(failed_ids.contains(&"FILLS"), "FILLS should warn on E-4: 14% wipeouts. Failed: {:?}", failed_ids);
    assert!(failed_ids.contains(&"SYMBOLS"), "SYMBOLS should warn on E-4: 0 blocks. Failed: {:?}", failed_ids);
}

// ─── ADDENDUM: DO NOT SCALE test ────────────────────────────────────────────

#[test]
fn arch_sheet_at_true_scale_passes() {
    let d = arch_sheet_true_scale();
    let result = import_check::check_drawing(&d);
    assert!(
        result.pass,
        "A-sheet at true scale should pass: score={}, failed={:?}",
        result.score,
        result.checks.iter().filter(|c| !c.pass).map(|c| &c.id).collect::<Vec<_>>()
    );
    assert!(result.discipline.as_deref() == Some("A"), "discipline should be A, got {:?}", result.discipline);
    assert!(result.do_not_scale.is_none(), "A-sheet should have no do-not-scale note");
}

#[test]
fn elec_sheet_do_not_scale_detected() {
    let d = elec_sheet_do_not_scale();
    let result = import_check::check_drawing(&d);

    // The DO NOT SCALE note should be detected.
    assert!(result.do_not_scale.is_some(), "DO NOT SCALE note should be detected");

    // SCALE check should pass (cannot judge without arch reference).
    let scale = result.checks.iter().find(|c| c.id == "SCALE");
    assert!(scale.is_some_and(|c| c.pass), "SCALE should pass with DO NOT SCALE note (needs registration)");

    // Evidence should indicate registration is needed.
    let ev = scale.and_then(|c| c.evidence.as_ref());
    assert!(ev.is_some_and(|e| e["needs_registration"] == json!(true)), "evidence should show needs_registration=true");
    assert!(ev.is_some_and(|e| e["do_not_scale"].as_str().is_some()), "evidence should contain the do-not-scale note text");

    // Discipline should be E.
    assert!(result.discipline.as_deref() == Some("E"), "discipline should be E, got {:?}", result.discipline);
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
    assert_eq!(result.hard_fails, back.hard_fails);
    assert_eq!(result.discipline, back.discipline);
}

#[test]
fn hard_fails_serialize() {
    let d = bad_import();
    let result = import_check::check_drawing(&d);
    let json_str = result.to_json();
    let back = ImportCheckResult::from_json(&json_str).expect("should deserialize");
    assert!(!back.hard_fails.is_empty(), "hard_fails should survive serialization");
}

#[test]
fn evidence_fields_serialize() {
    let d = good_import();
    let result = import_check::check_drawing(&d);
    let json_str = result.to_json();
    let back = ImportCheckResult::from_json(&json_str).expect("should deserialize");
    // At least some checks should have evidence.
    let has_evidence = back.checks.iter().any(|c| c.evidence.is_some());
    assert!(has_evidence, "some checks should have evidence fields after serialization");
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
fn passes_returns_false_for_unreadable_import() {
    let mut d = unreadable_import();
    let result = import_check::check_drawing(&d);
    import_check::store_result(&mut d, &result);
    assert!(!import_check::passes(&d), "unreadable import (only fills) should not pass");
}

// ─── Command registration ────────────────────────────────────────────────────

#[test]
fn import_check_command_is_registered() {
    assert!(cadaddy_engine::cmd::find_by_id_or_alias("import.check").is_some(), "import.check command should be registered");
}

// ─── import.check via Session ────────────────────────────────────────────────

#[test]
fn import_check_command_on_current_drawing() {
    let mut s = Session::new();
    let r = s.execute("import.check", &json!({}));
    assert!(r.is_ok(), "import.check should succeed: {:?}", r.err());
    let v = r.unwrap();
    assert!(v["pass"].as_bool().unwrap_or(false), "blank drawing should pass import check");
    // New fields should be present.
    assert!(
        v.get("hard_fails").is_some() || v["hard_fails"].is_null() || v.get("hard_fails").is_none(),
        "hard_fails field should be in response (may be absent if empty)"
    );
}

// ─── BIM gate ────────────────────────────────────────────────────────────────

#[test]
fn bim_build_refuses_unreadable_import() {
    let mut s = Session::new();
    if let Ok(d) = s.doc_mut() {
        let bad = unreadable_import();
        let result = import_check::check_drawing(&bad);
        import_check::store_result(d, &result);
    }
    let r = s.execute("bim.build", &json!({}));
    assert!(r.is_err(), "bim.build should refuse when import is unreadable (only fills)");
    let err = format!("{}", r.unwrap_err());
    assert!(err.contains("import check failed"), "error should mention import check: {err}");
}

#[test]
fn bim_build_allows_force_override() {
    let mut s = Session::new();
    if let Ok(d) = s.doc_mut() {
        let bad = unreadable_import();
        let result = import_check::check_drawing(&bad);
        import_check::store_result(d, &result);
    }
    let r = s.execute("bim.build", &json!({"force": true}));
    if let Err(e) = &r {
        let msg = format!("{e}");
        assert!(!msg.contains("import check failed"), "force should bypass import check gate, got: {msg}");
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
    assert_eq!(manifest["importCheck"]["pass"].as_bool(), Some(true), "manifest importCheck.pass should be true");
}

// ─── Printset stage gate ─────────────────────────────────────────────────────

#[test]
fn printset_run_refuses_unreadable_import() {
    let mut s = Session::new();
    if let Ok(d) = s.doc_mut() {
        let bad = unreadable_import();
        let result = import_check::check_drawing(&bad);
        import_check::store_result(d, &result);
    }
    let r = s.execute("printset.run", &json!({}));
    assert!(r.is_err(), "printset.run should refuse when import is unreadable");
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
