//! Undo keeps a memory budget: big edits drop the oldest steps instead of growing without limit,
//! and the newest step can always be undone.

use cadaddy_engine::Session;
use serde_json::json;

#[test]
fn big_edits_past_the_budget_drop_the_oldest_steps() {
    let mut s = Session::new();
    for i in 0..3000 {
        let x = f64::from(i);
        s.execute("line", &json!({"points": [[x, 0], [x, 1]]})).unwrap();
    }
    // Small budget: a MOVE of everything holds about the whole drawing's worth of memory.
    s.state_mut().unwrap().undo_ctl.budget = 1;
    let before = s.state().unwrap().undo.len();
    for _ in 0..5 {
        s.execute("select", &json!({"all": true})).unwrap();
        s.execute("move", &json!({"delta": [1, 0]})).unwrap();
    }
    let st = s.state().unwrap();
    assert!(st.undo.len() < before + 5, "steps dropped: {} -> {}", before, st.undo.len());
    assert!(!st.undo.is_empty(), "the newest step is always kept");
    assert!(st.undo.last().is_some_and(|u| u.bytes > 0), "a MOVE of everything holds memory");
    // The newest move can still be undone.
    s.execute("u", &json!({})).unwrap();
}

#[test]
fn small_edits_keep_every_step() {
    let mut s = Session::new();
    for i in 0..50 {
        let x = f64::from(i);
        s.execute("line", &json!({"points": [[x, 0], [x, 1]]})).unwrap();
    }
    assert_eq!(s.state().unwrap().undo.len(), 50);
}
