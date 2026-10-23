//! The print validator (Q321) on the synthetic hotel: a model true to the sheets passes clean,
//! every kind of planted error is caught (each where it was planted, with its source and a fix),
//! a scoped re-check finds it while looking at much less, and the checks are fast.

use cadaddy_mepfix::hotel::HotelSize;
use cadaddy_mepfix::printcheck::{PLANTED, model, plant, prints, prints_from_readers};
use cadaddy_walk3d::validate::{Failure, Model, Prints, Scope, Severity, new_failures, validate};

fn hotel(size: HotelSize) -> (Prints, Model) {
    let set = cadaddy_mepfix::generate_hotel(7, size).unwrap();
    (prints(&set.truth), model(&set.truth))
}

#[test]
fn a_true_model_of_the_hotel_passes_clean() {
    for size in [HotelSize::Small, HotelSize::Full] {
        let (p, m) = hotel(size);
        let r = validate(&p, &m, &Scope::all());
        assert!(r.pass, "{size:?}: {:#?}", r.failures.iter().take(10).collect::<Vec<_>>());
        assert!(r.failures.is_empty(), "no false alarms at all on the hotel");
        assert_eq!(r.score, 100.0);
        // Every check looked at something.
        for c in ["overlay", "counts", "dimensions", "tags", "connectivity", "physical", "provenance", "spec", "termination"] {
            assert!(r.checked.get(c).copied().unwrap_or(0) > 0, "{c}: {:?}", r.checked);
        }
        assert!(
            p.symbols.len() > 300 && p.dimensions.len() > 20 && m.runs.len() > 100,
            "{} {} {}",
            p.symbols.len(),
            p.dimensions.len(),
            m.runs.len()
        );
    }
}

fn caught(p: &Prints, base: &[Failure], m: &Model, kind: &str) -> Vec<Failure> {
    let r = validate(p, m, &Scope::all());
    let (new, _) = new_failures(base, &r.failures);
    new.into_iter().filter(|f| f.rule == kind).cloned().collect()
}

#[test]
fn every_kind_of_planted_error_is_caught() {
    for size in [HotelSize::Small, HotelSize::Full] {
        let (p, m) = hotel(size);
        let base = validate(&p, &m, &Scope::all()).failures;
        for kind in PLANTED {
            let mut bad = m.clone();
            let at = plant(&mut bad, &p, kind).unwrap_or_else(|| panic!("{size:?}: nothing to plant {kind} on"));
            let found = caught(&p, &base, &bad, kind);
            assert!(!found.is_empty(), "{size:?}: {kind} planted on {at} was not caught");
            // Element-level errors name the element they were planted on.
            let named = !["missing_in_model", "room_count", "schedule_quantity", "dimension_mismatch", "lug_mismatch"].contains(&kind);
            if named {
                assert!(found.iter().any(|f| f.element.as_deref() == Some(at.as_str())), "{kind}: {found:#?} should name {at}");
            }
            for f in &found {
                assert!(!f.fix.is_empty() && !f.expected.is_empty() && !f.found.is_empty(), "{f:#?}");
                assert!(f.id.starts_with("V-"), "{f:#?}");
            }
        }
    }
}

#[test]
fn failures_say_where_the_truth_comes_from() {
    let (p, m) = hotel(HotelSize::Small);
    let base = validate(&p, &m, &Scope::all()).failures;
    let one = |kind: &str| {
        let mut bad = m.clone();
        plant(&mut bad, &p, kind).unwrap();
        caught(&p, &base, &bad, kind).into_iter().next().unwrap()
    };
    // The sheet and the place on it.
    let f = one("misplaced");
    assert!(f.source.sheet.as_deref().is_some_and(|s| s.starts_with("E-")) && f.source.at.is_some(), "{f:#?}");
    assert_eq!(f.severity, Severity::Medium);
    // A schedule row.
    let f = one("schedule_quantity");
    assert!(f.source.row.as_deref().is_some_and(|r| r.starts_with("luminaire")), "{f:#?}");
    // A spec section.
    let f = one("spec_violation");
    assert_eq!(f.source.section.as_deref(), Some("26 05 33"), "{f:#?}");
    // Lugs are high severity.
    let f = one("lug_mismatch");
    assert_eq!(f.severity, Severity::High);
    assert!(f.found.contains("1/0") && f.expected.contains("#4"), "{f:#?}");
    // A dimension string.
    let f = one("dimension_mismatch");
    assert!(f.source.row.as_deref().is_some_and(|r| r.starts_with("dimension")) && f.found.contains("0'-3\""), "{f:#?}");
}

#[test]
fn ids_are_stable_and_a_fixed_error_goes_away() {
    let (p, m) = hotel(HotelSize::Small);
    let mut bad = m.clone();
    plant(&mut bad, &p, "wrong_height").unwrap();
    let a = validate(&p, &bad, &Scope::all());
    let b = validate(&p, &bad, &Scope::all());
    assert_eq!(a, b, "deterministic");
    let clean = validate(&p, &m, &Scope::all());
    let (new, fixed) = new_failures(&a.failures, &clean.failures);
    assert!(new.is_empty() && fixed == a.failures.len() && fixed > 0);
}

#[test]
fn a_scoped_recheck_finds_the_error_and_looks_at_less() {
    let (p, m) = hotel(HotelSize::Full);
    for kind in ["misplaced", "wrong_type", "off_wall", "duplicate", "spec_violation", "dangling", "untraced", "box_too_deep"] {
        let mut bad = m.clone();
        let at = plant(&mut bad, &p, kind).unwrap();
        let full = validate(&p, &bad, &Scope::all());
        let near = validate(&p, &bad, &Scope { elements: vec![at.clone()], ..Scope::default() });
        assert!(near.scoped);
        let ids = |fs: &[Failure]| fs.iter().filter(|f| f.rule == kind).map(|f| f.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(&near.failures), ids(&full.failures), "{kind}");
        assert!(!ids(&near.failures).is_empty(), "{kind}");
        let n = |r: &cadaddy_walk3d::validate::Report| r.checked.values().sum::<usize>();
        assert!(n(&near) * 10 < n(&full), "{kind}: {} vs {}", n(&near), n(&full));
    }
}

#[test]
fn outdoor_receptacles_must_be_weather_resistant() {
    let (p, mut m) = hotel(HotelSize::Small);
    let roof = p.rooms.iter().find(|r| r.exterior).unwrap();
    let mut e = m.elements.iter().find(|e| e.kind == "receptacle").unwrap().clone();
    e.id = "roof-recep".into();
    e.level = roof.level.clone();
    e.at = roof.outline[0];
    m.elements.push(e);
    let r = validate(&p, &m, &Scope { checks: vec![cadaddy_walk3d::validate::Check::Spec], ..Scope::default() });
    let f = r.failures.iter().find(|f| f.element.as_deref() == Some("roof-recep")).unwrap();
    assert_eq!(f.source.section.as_deref(), Some("26 27 26"));
    assert!(f.found.contains("dry"), "{f:#?}");
}

#[test]
fn the_checks_are_fast() {
    let (p, m) = hotel(HotelSize::Full);
    let t = std::time::Instant::now();
    for _ in 0..5 {
        let _ = validate(&p, &m, &Scope::all());
    }
    // ~1 300 items: well under a second each even in a debug build.
    assert!(t.elapsed().as_secs_f64() / 5.0 < 2.0, "{:?}", t.elapsed());
}

#[test]
fn prints_survive_json() {
    let (p, _) = hotel(HotelSize::Small);
    let text = serde_json::to_string(&p).unwrap();
    assert_eq!(Prints::from_json(&text).unwrap(), p.clone().capped());
}

// ---- MUST 4: Print-set feed (reader-built Prints) ----

#[test]
fn reader_built_prints_validate_a_true_model() {
    let set = cadaddy_mepfix::generate_hotel(7, HotelSize::Small).unwrap();
    let reader_prints = prints_from_readers(&set.truth);
    let m = model(&set.truth);
    let r = validate(&reader_prints, &m, &Scope::all());
    // Allow a small number of false alarms (pass_per_mille threshold).
    let threshold = reader_prints.symbols.len() / 10; // 10% tolerance for reader-vs-truth differences
    assert!(r.failures.len() <= threshold,
        "reader-built prints should validate a true model with at most {} failures, got {}: {:?}",
        threshold, r.failures.len(),
        r.failures.iter().take(5).map(|f| format!("{}: {}", f.rule, f.expected)).collect::<Vec<_>>());
    // Symbols should be non-trivial.
    assert!(reader_prints.symbols.len() > 100, "reader prints should have >100 symbols, got {}", reader_prints.symbols.len());
    assert!(reader_prints.rooms.len() > 5, "reader prints should have >5 rooms, got {}", reader_prints.rooms.len());
    assert!(reader_prints.schedule.len() > 3, "reader prints should have >3 schedule rows, got {}", reader_prints.schedule.len());
}

#[test]
fn reader_built_prints_catch_planted_errors() {
    let set = cadaddy_mepfix::generate_hotel(7, HotelSize::Small).unwrap();
    let reader_prints = prints_from_readers(&set.truth);
    let base_model = model(&set.truth);
    let base = validate(&reader_prints, &base_model, &Scope::all());
    // At least 5 planted errors should be caught.
    let target_kinds = ["missing_in_model", "misplaced", "schedule_quantity", "wrong_type", "box_too_deep", "lug_mismatch", "not_on_print"];
    let mut caught = 0;
    let mut caught_kinds = Vec::new();
    for kind in &target_kinds {
        let mut m = base_model.clone();
        if let Some(_id) = plant(&mut m, &reader_prints, kind) {
            let r = validate(&reader_prints, &m, &Scope::all());
            let (new, _) = new_failures(&base.failures, &r.failures);
            if new.iter().any(|f| f.rule == *kind) {
                caught += 1;
                caught_kinds.push(*kind);
            }
        }
    }
    assert!(caught >= 5, "reader-built prints should catch at least 5 planted errors, caught {caught} ({caught_kinds:?})");
}
