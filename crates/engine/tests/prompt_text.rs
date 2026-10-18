//! Prompt wording: one "or" and one keyword list, and keywords typed whole win over selection words.

use cadaddy_engine::Session;
use serde_json::json;

fn prompt(s: &Session) -> String {
    s.prompt_text()
}

#[test]
fn prompts_show_one_or_and_one_keyword_list() {
    let mut s = Session::new();
    s.execute("pline", &json!({"points": [[0, 0], [5, 0], [5, 5]]})).unwrap();
    s.execute("line", &json!({"points": [[0, 0], [9, 9]]})).unwrap();
    for cmd in ["pedit", "lengthen", "count", "blend"] {
        s.cmdline(cmd).unwrap();
        let p = prompt(&s);
        assert!(!p.contains(" or or ") && p.matches('[').count() <= 1, "{cmd}: {p}");
        s.cancel();
    }
}

#[test]
fn a_keyword_shortcut_wins_over_last() {
    let mut s = Session::new();
    s.execute("circle", &json!({"center": [0, 0], "radius": 1})).unwrap();
    s.cmdline("count").unwrap();
    // `L` is COUNT's List, not the Last object.
    s.cmdline("l").unwrap();
    assert!(s.running.is_none(), "{:?}", s.log.last());
}
