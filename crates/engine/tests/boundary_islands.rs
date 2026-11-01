//! BOUNDARY, HATCH and AUTOCONSTRAIN on repeated work (REPORT-promptfuzz #3): a pick over an area
//! an earlier BOUNDARY already traced takes each island once, so the work doesn't grow pick by pick.

use cadaddy_doc::EntityKind;
use cadaddy_engine::Session;
use serde_json::json;

fn polylines_added(s: &mut Session, p: [f64; 2]) -> usize {
    let before = s.doc().unwrap().model.len();
    s.execute("boundary", &json!({ "points": [p] })).unwrap();
    s.doc().unwrap().model.iter().skip(before).filter(|e| matches!(e.kind, EntityKind::LwPolyline(_))).count()
}

#[test]
fn a_second_pick_over_the_same_area_adds_no_copies_of_copies() {
    let mut s = Session::new();
    s.execute("rectang", &json!({ "p1": [0, 0], "p2": [20, 10] })).unwrap();
    s.execute("circle", &json!({ "center": [5, 5], "radius": 2 })).unwrap();
    s.execute("rectang", &json!({ "p1": [12, 3], "p2": [16, 7] })).unwrap();
    let first = polylines_added(&mut s, [9.0, 5.0]);
    assert_eq!(first, 3, "the outer boundary and two islands");
    // The same pick again: the earlier boundary's polylines lie on the same outlines.
    for _ in 0..5 {
        assert_eq!(polylines_added(&mut s, [9.0, 5.0]), first, "each island once, however many copies exist");
    }
}

#[test]
fn a_hatch_takes_a_duplicated_island_once() {
    let mut s = Session::new();
    s.execute("rectang", &json!({ "p1": [0, 0], "p2": [20, 10] })).unwrap();
    s.execute("circle", &json!({ "center": [5, 5], "radius": 2 })).unwrap();
    // A copy of the circle on top of it: twice as an island would cancel out (even-odd) and fill it.
    s.execute("circle", &json!({ "center": [5, 5], "radius": 2 })).unwrap();
    s.execute("hatch", &json!({ "points": [[12, 5]], "pattern": "SOLID" })).unwrap();
    let d = s.doc().unwrap();
    let h = d.model.iter().find_map(|e| if let EntityKind::Hatch(h) = &e.kind { Some(h.clone()) } else { None }).unwrap();
    assert_eq!(h.loops.len(), 2, "outer + one island");
}

#[test]
fn boundary_picks_on_the_sample_stay_fast() {
    // The promptfuzz seed 88 sequence: 3.6 s for one pick before; now each pick traces only the
    // objects there are, not a growing pile of copies (worst pick 37 ms, measured).
    let mut s = Session::new();
    s.open_drawing(cadaddy_engine::sample::default_sample(), "Bracket", None);
    let picks = [[5.6, 2.9], [13.8, 5.5], [2.6, 5.4], [5.0, 5.9], [0.1, 4.1], [5.9, 6.1], [2.9, 0.0], [8.3, 6.0], [12.2, 1.4], [10.4, 1.6]];
    let all = std::time::Instant::now();
    for p in picks {
        let t = std::time::Instant::now();
        let _ = s.execute("boundary", &json!({ "points": [p] }));
        assert!(t.elapsed().as_secs_f64() < 1.0, "pick {p:?} took {:?}", t.elapsed());
    }
    assert!(s.doc().unwrap().model.len() < 400, "{} objects: copies are piling up", s.doc().unwrap().model.len());
    assert!(all.elapsed().as_secs_f64() < 1.0, "the ten picks took {:?}", all.elapsed());
}

/// The sample's AUTOCONSTRAIN candidates and the constraints they come to, pinned.
const SAMPLE_CANDIDATES: u64 = 482;
const SAMPLE_CONSTRAINTS: usize = 60;

#[test]
fn autoconstrain_all_on_the_sample_is_fast_and_idempotent() {
    // 482 candidates, 60 kept: the implied ones are skipped without solving (4.3 s before).
    let mut s = Session::new();
    s.open_drawing(cadaddy_engine::sample::default_sample(), "Bracket", None);
    let hs: Vec<String> = s.doc().unwrap().model.iter().map(|e| e.handle.hex()).collect();
    let t = std::time::Instant::now();
    let r = s.execute("autoconstrain", &json!({ "handles": hs })).unwrap();
    let took = t.elapsed();
    assert_eq!(s.doc().unwrap().constraints.len(), SAMPLE_CONSTRAINTS);
    // The task's bar is 1 s: 0.56 s on the HP tree, but 1.1 s here in the debug test profile on a
    // slower Linux server, so the clock bound is 2 s (the old code took 4.3 s on the HP).
    assert!(took.as_secs_f64() < 2.0, "{took:?}");
    // Deterministic guard, whatever the hardware: most candidates are skipped as implied (359) and
    // only a fraction reaches the solver (123); without the skip all 482 were solved.
    let n = |k: &str| r[k].as_u64().unwrap();
    assert_eq!(n("candidates"), SAMPLE_CANDIDATES);
    assert_eq!(n("skippedImplied") + n("solved"), n("candidates"));
    assert!(n("solved") * 3 < n("candidates"), "{} of {} candidates solved: the implied skip is not working", n("solved"), n("candidates"));
    // Everything it would add now already holds.
    s.execute("autoconstrain", &json!({ "handles": hs })).unwrap();
    assert_eq!(s.doc().unwrap().constraints.len(), SAMPLE_CONSTRAINTS);
}

#[test]
fn autoconstrain_seed_70_sequence_is_fast() {
    // promptfuzz seed 70 on the sample: AUTOCONSTRAIN, `ALL` at the selection prompt, Enter
    // (about 7 s before the fix). Typed the way the fuzzer's `ALL` + Enter steps do it.
    let mut s = Session::new();
    s.open_drawing(cadaddy_engine::sample::default_sample(), "Bracket", None);
    let t = std::time::Instant::now();
    s.start("autoconstrain").unwrap();
    let _ = s.cmdline("ALL");
    let _ = s.input(cadaddy_engine::Input::Enter);
    assert!(t.elapsed().as_secs_f64() < 2.0, "{:?} (1.1 s measured on the Linux server, debug profile)", t.elapsed());
    assert!(s.running.is_none(), "the command should have finished: {}", s.prompt_text());
    assert_eq!(s.doc().unwrap().constraints.len(), SAMPLE_CONSTRAINTS);
}
