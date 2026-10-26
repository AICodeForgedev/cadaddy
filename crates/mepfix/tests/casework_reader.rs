//! Tests for the casework and plumbing fixture reader: reads casework, fixtures and the casework
//! / plumbing fixture schedule from architectural floor plan DXF sheets and scores them against
//! the truth (office seeds 1–10 and the hotel set), and the zones and in-wall clashes built
//! from what it read.

use cadaddy_doc::{Drawing, EntityKind};
use cadaddy_mepfix::casework_reader::{self, Reading};
use cadaddy_mepfix::hotel::HotelSize;
use cadaddy_mepfix::layout::Kind;
use cadaddy_mepfix::truth::{Fixture, Sheet, Truth};
use cadaddy_mepfix::{clash, draw, elec_reader, generate, score, truth_for};

/// The floor plan sheets of a planned set, drawn and read back from DXF: (sheet, drawing).
fn floor_plans(t: &Truth, st: &cadaddy_mepfix::style::Style, kinds: &[(String, Kind)]) -> Vec<(Sheet, Drawing)> {
    t.sheets
        .iter()
        .filter(|sh| sh.number.starts_with("A-10") && sh.title.contains("FLOOR PLAN"))
        .filter_map(|sh| {
            let (_, kind) = kinds.iter().find(|(n, _)| *n == sh.number)?;
            let (d, _, _) = draw::draw(t, st, sh, kind);
            let back = cadaddy_io::read_dxf(cadaddy_io::write_dxf(&d).as_bytes()).unwrap_or_else(|e| panic!("{}: {e}", sh.number));
            Some((sh.clone(), back))
        })
        .collect()
}

fn office(seed: u64) -> (Truth, Vec<(Sheet, Drawing)>) {
    let (t, st, kinds) = cadaddy_mepfix::office_planned(seed);
    let plans = floor_plans(&t, &st, &kinds);
    (t, plans)
}

fn hotel(seed: u64, size: HotelSize) -> (Truth, Vec<(Sheet, Drawing)>) {
    let (t, st) = cadaddy_mepfix::hotel_planned(seed, size);
    let kinds: Vec<(String, Kind)> = cadaddy_mepfix::layout::hotel_sheet_list().into_iter().map(|(n, _, _, k)| (n, k)).collect();
    let plans = floor_plans(&t, &st, &kinds);
    (t, plans)
}

/// Totals over sheets: casework matched by tag and position, and the scorer's counts.
#[derive(Default, Debug)]
struct Tally {
    truth: usize,
    found: usize,
    /// Truth items with a found item of the same tag whose centre is within 0.5 ft.
    tag_matched: usize,
    kind_ok: usize,
    height_err: f64,
    pos_err: f64,
    /// From `score::score_casework` (greedy by centre).
    tp: usize,
    fp: usize,
    fn_: usize,
}

impl Tally {
    fn add(&mut self, t: &Truth, sh: &Sheet, r: &Reading) {
        let level = sh.level.clone().unwrap_or_default();
        let found = casework_reader::to_found(&r.casework, sh);
        let s = score::score_casework(t, &level, &found, 3.0);
        self.tp += s.true_positives;
        self.fp += s.false_positives;
        self.fn_ += s.false_negatives;
        let truth: Vec<_> = t.casework.iter().filter(|c| c.level == level).collect();
        self.truth += truth.len();
        self.found += found.len();
        let mut used = vec![false; found.len()];
        for tr in truth {
            let tc = [(tr.min_ft[0] + tr.max_ft[0]) / 2.0, (tr.min_ft[1] + tr.max_ft[1]) / 2.0];
            let best = found
                .iter()
                .enumerate()
                .filter(|(i, f)| !used[*i] && f.tag == tr.tag)
                .map(|(i, f)| (i, ((f.min_ft[0] + f.max_ft[0]) / 2.0 - tc[0]).hypot((f.min_ft[1] + f.max_ft[1]) / 2.0 - tc[1])))
                .filter(|(_, d)| *d <= 0.5)
                .min_by(|a, b| a.1.total_cmp(&b.1));
            if let Some((i, d)) = best {
                used[i] = true;
                self.tag_matched += 1;
                self.pos_err += d;
                self.height_err += (found[i].height_in - tr.height_in).abs();
                if found[i].kind == tr.kind {
                    self.kind_ok += 1;
                }
            }
        }
    }

    fn kind_accuracy(&self) -> f64 {
        self.kind_ok as f64 / self.tag_matched.max(1) as f64
    }
    fn height_mae(&self) -> f64 {
        self.height_err / self.tag_matched.max(1) as f64
    }
    fn position_mae(&self) -> f64 {
        self.pos_err / self.tag_matched.max(1) as f64
    }
    fn precision(&self) -> f64 {
        self.tp as f64 / (self.tp + self.fp).max(1) as f64
    }
    fn recall(&self) -> f64 {
        self.tag_matched as f64 / self.truth.max(1) as f64
    }

    /// MUST 1 and 2's thresholds.
    fn assert_thresholds(&self, what: &str) {
        assert!(self.truth > 0, "{what}: no casework in the truth");
        assert!(self.kind_accuracy() >= 0.95, "{what}: kind accuracy {:.3} < 0.95 ({self:?})", self.kind_accuracy());
        assert!(self.height_mae() <= 1.0, "{what}: height MAE {:.2} in > 1.0 ({self:?})", self.height_mae());
        assert!(self.precision() >= 0.95, "{what}: precision {:.3} < 0.95 ({self:?})", self.precision());
        assert!(self.recall() >= 0.95, "{what}: recall {:.3} < 0.95 ({self:?})", self.recall());
        assert!(self.position_mae() <= 0.5, "{what}: position MAE {:.2} ft > 0.5 ({self:?})", self.position_mae());
    }
}

/// The office design generates casework (break rooms, toilets, conference rooms).
#[test]
fn office_has_casework() {
    for seed in 1..=10 {
        let t = truth_for(seed);
        assert!(!t.casework.is_empty(), "seed {seed}: office building should have casework");
    }
}

/// Casework items have valid fields: non-empty kind, level, room, and positive dimensions.
#[test]
fn casework_items_have_valid_fields() {
    for seed in 1..=10 {
        let t = truth_for(seed);
        for cw in &t.casework {
            assert!(!cw.kind.is_empty(), "seed {seed}: casework {}: empty kind", cw.id);
            assert!(!cw.level.is_empty(), "seed {seed}: casework {}: empty level", cw.id);
            assert!(cw.min_ft[0] < cw.max_ft[0], "seed {seed}: casework {}: min_x >= max_x", cw.id);
            assert!(cw.min_ft[1] < cw.max_ft[1], "seed {seed}: casework {}: min_y >= max_y", cw.id);
            assert!(cw.height_in > 0.0, "seed {seed}: casework {}: height_in <= 0", cw.id);
            assert!(cw.depth_in > 0.0, "seed {seed}: casework {}: depth_in <= 0", cw.id);
        }
    }
}

/// The perfect casework scorer round-trips: passing truth back gets 100% precision and recall.
#[test]
fn perfect_casework_scorer_round_trips() {
    for seed in 1..=10 {
        let t = truth_for(seed);
        let perfect = score::perfect_casework(&t);
        for level in ["1", "2"] {
            let s = score::score_casework(&t, level, &perfect, 5.0);
            let truth_n = t.casework.iter().filter(|c| c.level == level).count();
            assert_eq!(
                s.true_positives, truth_n,
                "seed {seed} level {level}: perfect scorer should find all {truth_n} items, got {}",
                s.true_positives
            );
            assert_eq!(s.false_positives, 0, "seed {seed} level {level}: perfect scorer should have 0 FP");
            assert_eq!(s.false_negatives, 0, "seed {seed} level {level}: perfect scorer should have 0 FN");
            if truth_n > 0 {
                assert!((s.precision - 1.0).abs() < 1e-9, "seed {seed} level {level}: precision should be 1.0");
                assert!((s.recall - 1.0).abs() < 1e-9, "seed {seed} level {level}: recall should be 1.0");
                assert!((s.kind_accuracy - 1.0).abs() < 1e-9, "seed {seed} level {level}: kind_accuracy should be 1.0");
                assert!(s.height_mae_in < 1e-9, "seed {seed} level {level}: height MAE should be 0");
                assert!(s.position_mae_ft < 1e-9, "seed {seed} level {level}: position MAE should be 0");
            }
        }
    }
}

/// MUST 1 and 2 on the office set, seeds 1–10: every item (also the base cabinet, countertop and
/// upper cabinet drawn on one footprint, and the sink in their run) gets its own tag and height;
/// the upper cabinet's inner line is no item. Kind accuracy ≥ 0.95, height MAE ≤ 1 in,
/// precision and recall ≥ 0.95, position MAE ≤ 0.5 ft.
#[test]
fn office_reader_meets_thresholds_seeds_1_to_10() {
    let mut all = Tally::default();
    for seed in 1..=10 {
        let (t, plans) = office(seed);
        assert!(!plans.is_empty(), "seed {seed}: no floor plans");
        let mut tally = Tally::default();
        for (sh, d) in &plans {
            let r = casework_reader::read(d);
            tally.add(&t, sh, &r);
            all.add(&t, sh, &r);
        }
        tally.assert_thresholds(&format!("office seed {seed}"));
    }
    all.assert_thresholds("office seeds 1-10");
    println!(
        "office seeds 1-10 casework: truth {} found {} | kind {:.3} height MAE {:.2} in | precision {:.3} recall {:.3} | position MAE {:.3} ft",
        all.truth,
        all.found,
        all.kind_accuracy(),
        all.height_mae(),
        all.precision(),
        all.recall(),
        all.position_mae()
    );
}

/// MUST 1: a break room's stacked base cabinet, countertop and upper cabinet come back as three
/// items with their own tags, kinds and heights (34", 36", 42"), not three copies of the first.
#[test]
fn stacked_items_keep_their_own_tag_and_height() {
    let mut checked = 0;
    for seed in 1..=10 {
        let (t, plans) = office(seed);
        for (sh, d) in &plans {
            let found = casework_reader::to_found(&casework_reader::read(d).casework, sh);
            let level = sh.level.clone().unwrap_or_default();
            for bc in t.casework.iter().filter(|c| c.level == level && c.tag == "BC-1") {
                let same: Vec<_> = found
                    .iter()
                    .filter(|f| {
                        (f.min_ft[0] - bc.min_ft[0]).abs() < 0.1
                            && (f.min_ft[1] - bc.min_ft[1]).abs() < 0.1
                            && (f.max_ft[0] - bc.max_ft[0]).abs() < 0.1
                            && (f.max_ft[1] - bc.max_ft[1]).abs() < 0.1
                    })
                    .collect();
                let mut got: Vec<(String, String, f64)> = same.iter().map(|f| (f.tag.clone(), f.kind.clone(), f.height_in)).collect();
                got.sort_by(|a, b| a.0.cmp(&b.0));
                assert_eq!(
                    got,
                    vec![
                        ("BC-1".into(), "base_cabinet".into(), 34.0),
                        ("CT-1".into(), "counter".into(), 36.0),
                        ("UC-1".into(), "upper_cabinet".into(), 42.0)
                    ],
                    "seed {seed} {}: the break room's footprint",
                    sh.number
                );
                checked += 1;
            }
        }
    }
    assert!(checked >= 5, "only {checked} break rooms looked at");
}

/// MUST 2: no read item is another's decoration (a rectangle inset the same on every side).
#[test]
fn inner_decoration_rectangles_are_not_items() {
    for seed in 1..=10 {
        let (t, plans) = office(seed);
        for (sh, d) in &plans {
            let r = casework_reader::read(d);
            for a in &r.casework {
                for b in &r.casework {
                    let ins = [a.min_in[0] - b.min_in[0], a.min_in[1] - b.min_in[1], b.max_in[0] - a.max_in[0], b.max_in[1] - a.max_in[1]];
                    let uniform = ins.iter().all(|v| *v > 0.2) && ins.iter().all(|v| (v - ins[0]).abs() < 0.5);
                    assert!(!uniform, "seed {seed} {}: {} at {:?} is the inner line of {}", sh.number, a.tag, a.min_in, b.tag);
                }
            }
            // Upper cabinets are drawn with an inner rectangle: as many items as the truth has.
            let level = sh.level.clone().unwrap_or_default();
            let truth_n = t.casework.iter().filter(|c| c.level == level).count();
            assert_eq!(r.casework.len(), truth_n, "seed {seed} {}: items", sh.number);
        }
    }
}

/// MUST 1–2 without the drawing order: every tag and note moved to the end of the drawing (as
/// another CAD program may save them), reversed — the tags go to the nearest item still without
/// one and the schedule names each, so the thresholds still hold.
#[test]
fn texts_drawn_last_still_meet_thresholds() {
    let mut tally = Tally::default();
    for seed in [1, 4, 7] {
        let (t, plans) = office(seed);
        for (sh, d) in &plans {
            let mut moved = d.clone();
            let texts: Vec<_> = moved
                .model
                .iter()
                .filter(|e| {
                    matches!(e.kind, EntityKind::Text(_))
                        && (e.common.layer.eq_ignore_ascii_case("CASEWORK") || e.common.layer.eq_ignore_ascii_case("TEXT"))
                })
                .map(|e| (e.handle, e.common.clone(), e.kind.clone()))
                .collect();
            for (h, _, _) in &texts {
                moved.remove_entity(*h);
            }
            for (_, common, kind) in texts.into_iter().rev() {
                let _ = moved.add(&cadaddy_doc::Space::Model, common, kind);
            }
            tally.add(&t, sh, &casework_reader::read(&moved));
        }
    }
    tally.assert_thresholds("texts drawn last");
}

/// MUST 3: plumbing fixtures (and the water heaters) are read from the office A-plans, seeds
/// 1–10: precision and recall ≥ 0.95, kind accuracy ≥ 0.95, position MAE ≤ 0.5 ft.
#[test]
fn office_fixtures_are_read_seeds_1_to_10() {
    let mut s_all = (0usize, 0usize, 0usize, 0usize, 0.0f64);
    for seed in 1..=10 {
        let (t, plans) = office(seed);
        for (sh, d) in &plans {
            let level = sh.level.clone().unwrap_or_default();
            let r = casework_reader::read(d);
            let found = casework_reader::fixtures_to_found(&r.fixtures, sh);
            let s = casework_reader::score_fixtures(&t, &level, &found, 3.0);
            s_all.0 += s.true_positives;
            s_all.1 += s.false_positives;
            s_all.2 += s.false_negatives;
            s_all.3 += (s.kind_accuracy * s.true_positives as f64).round() as usize;
            s_all.4 += s.position_mae_ft * s.true_positives as f64;
            assert!(s.true_positives > 0 || casework_reader::truth_fixtures(&t, &level).is_empty(), "seed {seed} {}: nothing read", sh.number);
        }
    }
    let (tp, fp, fn_, kind_ok, pos) = s_all;
    let precision = tp as f64 / (tp + fp).max(1) as f64;
    let recall = tp as f64 / (tp + fn_).max(1) as f64;
    let kind = kind_ok as f64 / tp.max(1) as f64;
    let mae = pos / tp.max(1) as f64;
    println!(
        "office seeds 1-10 fixtures: tp {tp} fp {fp} fn {fn_} | precision {precision:.3} recall {recall:.3} kind {kind:.3} position MAE {mae:.3} ft"
    );
    assert!(tp >= 40, "only {tp} fixtures matched");
    assert!(precision >= 0.95 && recall >= 0.95, "precision {precision:.3} recall {recall:.3}");
    assert!(kind >= 0.95, "kind accuracy {kind:.3}");
    assert!(mae <= 0.5, "position MAE {mae:.3} ft");
}

/// MUST 3, every kind: the office truth with a sink, urinal, shower, mop sink, bathtub and floor
/// drain added to its rooms (the generator's office has water closets, lavatories and water
/// heaters only) is drawn and read back: every kind is found, scored against that truth.
#[test]
fn every_fixture_kind_is_read() {
    let (mut t, st, kinds) = cadaddy_mepfix::office_planned(3);
    let rooms: Vec<_> =
        t.building.rooms.iter().filter(|r| r.level == "1" && r.max_ft[0] - r.min_ft[0] > 9.0 && r.max_ft[1] - r.min_ft[1] > 9.0).cloned().collect();
    let added = [("sink", "S-1"), ("urinal", "UR-1"), ("shower", "SH-1"), ("mop_sink", "MS-1"), ("bathtub", "BT-1"), ("floor_drain", "FD-1")];
    for (i, (kind, tag)) in added.iter().enumerate() {
        let rm = &rooms[i % rooms.len()];
        // Spread out inside the room, clear of each other.
        let at = [rm.min_ft[0] + 3.5 + 2.0 * (i / rooms.len()) as f64, rm.min_ft[1] + 4.0];
        t.fixtures.push(Fixture {
            id: format!("FX{i}"),
            tag: (*tag).into(),
            kind: (*kind).into(),
            level: "1".into(),
            room: rm.number.clone(),
            at_ft: [at[0], at[1], 0.0],
            nodes: vec![],
            wall: None,
        });
    }
    let plans = floor_plans(&t, &st, &kinds);
    let (sh, d) = plans.iter().find(|(s, _)| s.level.as_deref() == Some("1")).expect("level 1 floor plan");
    let r = casework_reader::read(d);
    let found = casework_reader::fixtures_to_found(&r.fixtures, sh);
    let s = casework_reader::score_fixtures(&t, "1", &found, 3.0);
    println!("every kind: {s:?}");
    for (kind, tag) in added.iter().copied().chain([("water_closet", "WC-1"), ("lavatory", "LAV-1"), ("water_heater", "WH-1")]) {
        let f = found.iter().find(|f| f.tag == tag).unwrap_or_else(|| panic!("{tag} not read: {found:?}"));
        assert_eq!(f.kind, kind, "{tag}");
        // Its schedule row named the kind.
        let row = r.schedule.iter().find(|x| x.tag == tag).unwrap_or_else(|| panic!("{tag} not in the schedule"));
        assert_eq!(row.kind, kind, "{tag}'s schedule row");
    }
    assert!(s.precision >= 0.95 && s.recall >= 0.95, "{s:?}");
    assert!(s.kind_accuracy >= 0.95, "{s:?}");
    assert!(s.position_mae_ft <= 0.5, "{s:?}");
}

/// MUST 4: the floor plans carry the casework and plumbing fixture schedule; every plan tag
/// resolves to a schedule row, and the items' kind and height are the row's.
#[test]
fn every_plan_tag_resolves_to_a_schedule_row() {
    for seed in 1..=10 {
        let (_, plans) = office(seed);
        for (sh, d) in &plans {
            let r = casework_reader::read(d);
            assert!(r.schedule.len() >= 5, "seed {seed} {}: schedule rows {:?}", sh.number, r.schedule);
            assert!(r.unscheduled.is_empty(), "seed {seed} {}: tags without a row: {:?}", sh.number, r.unscheduled);
            for c in &r.casework {
                let row = r.schedule.iter().find(|x| x.tag == c.tag).unwrap_or_else(|| panic!("seed {seed}: {} has no row", c.tag));
                assert_eq!(c.status, "schedule", "seed {seed}: {}", c.tag);
                assert_eq!(c.kind, row.kind, "seed {seed}: {} kind", c.tag);
                assert_eq!(Some(c.height_in), row.height_in, "seed {seed}: {} height", c.tag);
                assert!(c.spec_section.is_some(), "seed {seed}: {} spec", c.tag);
            }
            for f in &r.fixtures {
                assert_eq!(f.status, "schedule", "seed {seed}: fixture {}", f.tag);
            }
        }
    }
}

/// MUST 4: kind and height come from the schedule: change BC-1's row (description and height)
/// and the plan's BC-1 items follow it; drop the schedule and they fall back to the plan's notes.
#[test]
fn kind_and_height_follow_the_schedule_row() {
    let (_, plans) = office(1);
    let (sh, d) = plans.iter().find(|(_, d)| casework_reader::read(d).casework.iter().any(|c| c.tag == "BC-1")).expect("a plan with a break room");
    // BC-1's row: the cells right of the tag on its baseline.
    let mut edited = d.clone();
    let row_y = edited
        .model
        .iter()
        .filter(|e| e.common.layer.eq_ignore_ascii_case(casework_reader::SCHEDULE_LAYER))
        .find_map(|e| match &e.kind {
            EntityKind::Text(t) if t.value == "BC-1" => Some(t.insert.y),
            _ => None,
        })
        .expect("BC-1 row");
    let ids: Vec<_> = edited
        .model
        .iter()
        .filter(|e| e.common.layer.eq_ignore_ascii_case(casework_reader::SCHEDULE_LAYER))
        .filter_map(|e| match &e.kind {
            EntityKind::Text(t) if (t.insert.y - row_y).abs() < 1e-6 && (t.value == "BASE CABINET" || t.value.ends_with("AFF")) => Some(e.handle),
            _ => None,
        })
        .collect();
    assert_eq!(ids.len(), 2, "description and height cells");
    for h in ids {
        edited
            .modify_entity(h, |e| {
                if let EntityKind::Text(t) = &mut e.kind {
                    t.value = if t.value.ends_with("AFF") { "31\" AFF".into() } else { "VANITY".into() };
                }
            })
            .expect("edit");
    }
    let r = casework_reader::read(&edited);
    let bc: Vec<_> = r.casework.iter().filter(|c| c.tag == "BC-1").collect();
    assert!(!bc.is_empty());
    for c in &bc {
        assert_eq!((c.kind.as_str(), c.height_in, c.status.as_str()), ("vanity", 31.0, "schedule"), "{} on {}", c.tag, sh.number);
    }
    // Without a schedule: the plan's own note and tag.
    let mut bare = d.clone();
    let sched: Vec<_> =
        bare.model.iter().filter(|e| e.common.layer.eq_ignore_ascii_case(casework_reader::SCHEDULE_LAYER)).map(|e| e.handle).collect();
    for h in sched {
        bare.remove_entity(h);
    }
    let r = casework_reader::read(&bare);
    assert!(r.schedule.is_empty() && r.unscheduled.is_empty());
    for c in r.casework.iter().filter(|c| c.tag == "BC-1") {
        assert_eq!((c.kind.as_str(), c.height_in, c.status.as_str()), ("base_cabinet", 34.0, "note"));
    }
}

/// MUST 7: the reader on the hotel set (vanities, wardrobes, kitchen and front desk casework;
/// water closets, lavatories, tubs) meets the thresholds of MUST 1–2, and reads its fixtures.
#[test]
fn hotel_reader_meets_thresholds() {
    for (seed, size) in [(1, HotelSize::Small), (2, HotelSize::Small), (3, HotelSize::Full)] {
        let (t, plans) = hotel(seed, size);
        assert!(!plans.is_empty(), "hotel {seed}: no floor plans");
        let mut tally = Tally::default();
        let (mut ftp, mut ffp, mut ffn) = (0, 0, 0);
        for (sh, d) in &plans {
            let r = casework_reader::read(d);
            tally.add(&t, sh, &r);
            let level = sh.level.clone().unwrap_or_default();
            let s = casework_reader::score_fixtures(&t, &level, &casework_reader::fixtures_to_found(&r.fixtures, sh), 3.0);
            ftp += s.true_positives;
            ffp += s.false_positives;
            ffn += s.false_negatives;
            assert!(r.unscheduled.is_empty(), "hotel {seed} {}: {:?}", sh.number, r.unscheduled);
        }
        let kinds: std::collections::BTreeSet<&str> = t.casework.iter().map(|c| c.kind.as_str()).collect();
        assert!(kinds.contains("vanity") && kinds.contains("wardrobe"), "hotel {seed}: {kinds:?}");
        tally.assert_thresholds(&format!("hotel {seed} {size:?}"));
        let (p, rc) = (ftp as f64 / (ftp + ffp).max(1) as f64, ftp as f64 / (ftp + ffn).max(1) as f64);
        println!(
            "hotel {seed} {size:?}: casework truth {} kind {:.3} height MAE {:.2} precision {:.3} recall {:.3} position MAE {:.3} | fixtures tp {ftp} precision {p:.3} recall {rc:.3}",
            tally.truth,
            tally.kind_accuracy(),
            tally.height_mae(),
            tally.precision(),
            tally.recall(),
            tally.position_mae()
        );
        assert!(ftp > 0 && p >= 0.95 && rc >= 0.95, "hotel {seed}: fixtures precision {p:.3} recall {rc:.3}");
    }
}

/// MUST 6 (Q288): a counter receptacle zone over the backsplash the reader found, from reader
/// output only: its stretch is the read backsplash's, its band over the read counter top; a
/// receptacle the device reader put on that wall takes the zone. Moving the backsplash on the
/// sheet moves the zone.
#[test]
fn counter_receptacle_zone_over_the_read_backsplash() {
    let (t, plans) = office(1);
    let (sh, d) = plans.iter().find(|(_, d)| casework_reader::read(d).casework.iter().any(|c| c.tag == "BS-1")).expect("a break room");
    let found = casework_reader::to_found(&casework_reader::read(d).casework, sh);
    let zones = elec_reader::counter_receptacle_zones(&t.building.walls, &found);
    let z = zones.iter().find(|z| z.backsplash == "BS-1").expect("a zone over BS-1");
    let bs = found.iter().find(|f| f.tag == "BS-1").expect("read BS-1");
    let truth_bs = t.casework.iter().find(|c| c.tag == "BS-1" && Some(&c.level) == sh.level.as_ref()).expect("truth BS-1");
    assert!((z.zone_ft[0] - bs.min_ft[0]).abs() < 1e-6 && (z.zone_ft[1] - bs.max_ft[0]).abs() < 1e-6, "{z:?} vs {bs:?}");
    assert!((z.zone_ft[0] - truth_bs.min_ft[0]).abs() < 0.5 && (z.zone_ft[1] - truth_bs.max_ft[0]).abs() < 0.5, "{z:?} vs the truth");
    // CT-1 is 36" with an 18" (full-height) backsplash: 38"–56".
    assert_eq!((z.counter.as_str(), z.height_in), ("CT-1", [38.0, 56.0]), "{z:?}");
    let wall = t.wall(&z.wall).expect("its wall");
    assert!((wall.from_ft[1] - wall.to_ft[1]).abs() < 1e-6 && (wall.from_ft[1] - truth_bs.min_ft[1]).abs() < 1.5, "the backsplash's wall: {wall:?}");
    // A receptacle the E-sheet reader placed by the 12 ft rule over that counter.
    let mut devices = vec![elec_reader::FoundDevice {
        level: bs.level.clone(),
        kind: "receptacle".into(),
        at_ft: [(bs.min_ft[0] + bs.max_ft[0]) / 2.0 + 0.5, wall.from_ft[1], 18.0],
        mounting_in: 18.0,
        height_source: "rule".into(),
        zone: Some(elec_reader::DeviceZone {
            room: String::new(),
            wall_side: elec_reader::WallSide::South,
            axis: elec_reader::ZoneAxis::X,
            range_ft: [wall.from_ft[0].min(wall.to_ft[0]), wall.from_ft[0].max(wall.to_ft[0])],
            height_in: [14.0, 22.0],
            field_decision: true,
        }),
        ..Default::default()
    }];
    assert_eq!(elec_reader::apply_counter_zones(&mut devices, &t.building.walls, &zones), 1);
    assert_eq!(devices[0].zone.as_ref().map(|z| z.range_ft), Some(z.zone_ft));
    assert_eq!((devices[0].mounting_in, devices[0].height_source.as_str()), (38.0, "casework"));
    // Moved 2 ft east on the sheet, the read backsplash takes its zone along.
    let mut moved = found.clone();
    for f in moved.iter_mut().filter(|f| f.tag == "BS-1") {
        f.min_ft[0] += 2.0;
        f.max_ft[0] += 2.0;
    }
    let z2 = elec_reader::counter_receptacle_zones(&t.building.walls, &moved).into_iter().find(|z| z.backsplash == "BS-1").expect("zone");
    assert!((z2.zone_ft[0] - z.zone_ft[0] - 2.0).abs() < 1e-6, "{z2:?}");
}

/// MUST 6 (Q292): an in-wall clash behind the sink the reader found, from reader output: a
/// receptacle in the wall behind the read SK-1 clashes with its assumed rough-in; the truth's
/// casework plays no part (cleared, the clash stays); the read sink moved away, it goes.
#[test]
fn sink_clash_from_read_items() {
    let (mut t, plans) = office(1);
    let (sh, d) = plans.iter().find(|(_, d)| casework_reader::read(d).casework.iter().any(|c| c.tag == "SK-1")).expect("a break room");
    let r = casework_reader::read(d);
    let read = casework_reader::plumbing_items(&r, sh);
    let sink = read.iter().find(|p| p.tag == "SK-1").expect("the read sink");
    assert_eq!(sink.kind, "sink");
    let c = [(sink.min_ft[0] + sink.max_ft[0]) / 2.0, (sink.min_ft[1] + sink.max_ft[1]) / 2.0];
    let wall = t
        .building
        .walls
        .iter()
        .filter(|w| {
            w.level == sink.level
                && (w.from_ft[1] - w.to_ft[1]).abs() < 1e-6
                && c[0] >= w.from_ft[0].min(w.to_ft[0])
                && c[0] <= w.from_ft[0].max(w.to_ft[0])
        })
        .min_by(|a, b| (a.from_ft[1] - c[1]).abs().total_cmp(&(b.from_ft[1] - c[1]).abs()))
        .expect("the wall behind")
        .clone();
    // A receptacle at 18" AFF right behind the sink's waste stub.
    t.devices.push(cadaddy_mepfix::truth::Device {
        id: "DEV-T".into(),
        tag: "R-T".into(),
        kind: "receptacle".into(),
        level: sink.level.clone(),
        wall: wall.id.clone(),
        at_ft: [c[0] + 0.1, wall.from_ft[1]],
        aff_in: 18.0,
        box_in: [2.0, 4.0, 2.5],
        ..Default::default()
    });
    let hits = clash::casework_clashes(&t, &read, 3.0);
    let hit = hits.iter().find(|x| x.a == "DEV-T").unwrap_or_else(|| panic!("no clash: {hits:?}"));
    assert_eq!((hit.kind.as_str(), hit.b.as_str(), hit.wall.as_deref()), ("in_wall_casework", sink.id.as_str(), Some(wall.id.as_str())));
    assert!(hit.description.as_deref().is_some_and(|s| s.contains("sink SK-1") && s.contains("R-T")), "{hit:?}");
    // The truth's casework is not what it reads.
    let mut no_truth = t.clone();
    no_truth.casework.clear();
    assert!(clash::casework_clashes(&no_truth, &read, 3.0).iter().any(|x| x.a == "DEV-T"));
    // The read sink moved 5 ft along its wall: no clash with the receptacle.
    let mut moved = read.clone();
    for p in moved.iter_mut().filter(|p| p.tag == "SK-1") {
        p.min_ft[0] += 5.0;
        p.max_ft[0] += 5.0;
    }
    assert!(!clash::casework_clashes(&t, &moved, 3.0).iter().any(|x| x.a == "DEV-T"), "moved away");
    // A dishwasher read beside it: its box under the counter clashes with the receptacle too.
    let dw = clash::ReadPlumbing { id: "A:DW-1#1".into(), kind: "dishwasher".into(), tag: "DW-1".into(), ..sink.clone() };
    assert!(clash::casework_clashes(&t, &[dw], 3.0).iter().any(|x| x.a == "DEV-T" && x.b == "A:DW-1#1"));
}

/// Hostile drawings: zero-size and huge rectangles, NaN points and text, 10 000 rectangles and
/// tags — no panic, bounded time, nothing made up from the junk.
#[test]
fn hostile_drawings_never_panic() {
    use cadaddy_doc::{Common, LwPolyline, Space, Text};
    use cadaddy_geom::{PolyVertex, Vec2, Vec3};
    let mut d = Drawing::new_imperial();
    let rect = |a: [f64; 2], b: [f64; 2]| {
        let v = |x: f64, y: f64| PolyVertex { p: Vec2::new(x, y), bulge: 0.0, start_width: 0.0, end_width: 0.0 };
        EntityKind::LwPolyline(LwPolyline {
            vertices: vec![v(a[0], a[1]), v(b[0], a[1]), v(b[0], b[1]), v(a[0], b[1])],
            closed: true,
            const_width: 0.0,
            elevation: 0.0,
            plinegen: false,
        })
    };
    let text = |s: &str, at: [f64; 2]| {
        EntityKind::Text(Text {
            insert: Vec3::new(at[0], at[1], 0.0),
            align_pt: None,
            height: 6.0,
            value: s.into(),
            rotation: 0.0,
            width_factor: 1.0,
            oblique: 0.0,
            style: "Standard".into(),
            halign: cadaddy_doc::HAlign::Left,
            valign: cadaddy_doc::VAlign::Baseline,
        })
    };
    let on = |l: &str| Common { layer: l.into(), ..Common::default() };
    for (a, b) in [([0.0, 0.0], [0.0, 0.0]), ([f64::NAN, 0.0], [5.0, 5.0]), ([0.0, 0.0], [f64::INFINITY, 1e300]), ([-1e300, -1e300], [1e300, 1e300])]
    {
        let _ = d.add(&Space::Model, on("CASEWORK"), rect(a, b));
    }
    for i in 0..10_000 {
        let x = (i % 100) as f64 * 40.0;
        let y = (i / 100) as f64 * 40.0;
        let _ = d.add(&Space::Model, on("CASEWORK"), rect([x, y], [x + 30.0, y + 20.0]));
        let _ = d.add(&Space::Model, on("CASEWORK"), text("BC-1", [x + 15.0, y + 10.0]));
    }
    let _ = d.add(&Space::Model, on("CASEWORK"), text("BC-1", [f64::NAN, f64::NAN]));
    let _ = d.add(&Space::Model, on(casework_reader::FIXTURE_LAYER), text("WC-1", [f64::NAN, 1.0]));
    let _ = d.add(&Space::Model, on(casework_reader::SCHEDULE_LAYER), text("TAG", [f64::INFINITY, 0.0]));
    let t0 = std::time::Instant::now();
    let r = casework_reader::read(&d);
    assert!(t0.elapsed().as_secs_f64() < 10.0, "took {:?}", t0.elapsed());
    assert_eq!(r.casework.len(), 10_000, "only the real rectangles");
    assert!(r.casework.iter().all(|c| c.tag == "BC-1" && c.min_in.iter().chain(c.max_in.iter()).all(|v| v.is_finite())));
    assert!(r.fixtures.is_empty() && r.schedule.is_empty());
}

/// Break rooms always have base cabinets and a counter.
#[test]
fn break_rooms_have_cabinets_and_counter() {
    for seed in 1..=10 {
        let t = truth_for(seed);
        let break_rooms: Vec<_> = t.building.rooms.iter().filter(|r| r.name == "BREAK ROOM").collect();
        for rm in &break_rooms {
            let room_cw: Vec<_> = t.casework.iter().filter(|c| c.room == rm.number && c.level == rm.level).collect();
            let has_base = room_cw.iter().any(|c| c.kind == "base_cabinet");
            let has_counter = room_cw.iter().any(|c| c.kind == "counter");
            assert!(has_base, "seed {seed}: break room {} on level {} has no base cabinet", rm.number, rm.level);
            assert!(has_counter, "seed {seed}: break room {} on level {} has no counter", rm.number, rm.level);
        }
    }
}

/// Casework items are placed inside rooms (bounding-box containment check).
#[test]
fn casework_is_inside_rooms() {
    for seed in 1..=5 {
        let t = truth_for(seed);
        for cw in &t.casework {
            let room = t.building.rooms.iter().find(|r| r.number == cw.room && r.level == cw.level);
            if let Some(rm) = room {
                let cx = (cw.min_ft[0] + cw.max_ft[0]) / 2.0;
                let cy = (cw.min_ft[1] + cw.max_ft[1]) / 2.0;
                assert!(
                    cx >= rm.min_ft[0] - 1.0 && cx <= rm.max_ft[0] + 1.0 && cy >= rm.min_ft[1] - 1.0 && cy <= rm.max_ft[1] + 1.0,
                    "seed {seed}: casework {} ({}) at ({cx:.1}, {cy:.1}) is outside room {}",
                    cw.id,
                    cw.kind,
                    cw.room
                );
            }
        }
    }
}

/// Toilet rooms have vanities.
#[test]
fn toilet_rooms_have_vanities() {
    for seed in 1..=10 {
        let t = truth_for(seed);
        let toilets: Vec<_> = t
            .building
            .rooms
            .iter()
            .filter(|r| r.name.contains("TOILET") || r.name.contains("REST") || r.name.contains("MEN") || r.name.contains("WOMEN"))
            .collect();
        for rm in &toilets {
            let has_vanity = t.casework.iter().any(|c| c.room == rm.number && c.level == rm.level && c.kind == "vanity");
            assert!(has_vanity, "seed {seed}: toilet room {} ({}) on level {} has no vanity", rm.number, rm.name, rm.level);
        }
    }
}

/// The generated set's own DXF files (what `generate` writes) read the same.
#[test]
fn generated_set_files_read() {
    let set = generate(5).unwrap_or_else(|e| panic!("generate failed: {e}"));
    let t = &set.truth;
    let mut tally = Tally::default();
    for sh in t.sheets.iter().filter(|s| s.number.starts_with("A-10") && s.title.contains("FLOOR PLAN")) {
        let bytes = set.file(&sh.dxf).unwrap_or_else(|| panic!("missing DXF {}", sh.dxf));
        let d = cadaddy_io::read_dxf(bytes).unwrap_or_else(|e| panic!("read_dxf failed: {e}"));
        tally.add(t, sh, &casework_reader::read(&d));
    }
    tally.assert_thresholds("generated seed 5");
}
