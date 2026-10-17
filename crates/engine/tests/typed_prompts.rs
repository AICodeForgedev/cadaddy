//! Commands that had only a JSON form prompt when typed (battle test 3): LTSCALE, LIMITS, BASE,
//! COLOR, LWEIGHT, GETVAR, FIND, LAYISO, LAYMCUR, LAYMCH, BREAKATPOINT.

use cadcraft_engine::doc::EntityKind;
use cadcraft_engine::{Input, Session};
use serde_json::json;

fn typed(s: &mut Session, inputs: &[&str]) {
    for i in inputs {
        s.cmdline(i).unwrap_or_else(|e| panic!("{i}: {e}"));
    }
    assert!(s.running.is_none(), "still running after {inputs:?}: {}", s.prompt_text());
}

#[test]
fn bare_commands_prompt_for_what_they_need() {
    let mut s = Session::new();
    typed(&mut s, &["ltscale", "2.5"]);
    assert_eq!(cadcraft_engine::sysvars::get(&s, "LTSCALE"), Some(json!(2.5)));
    typed(&mut s, &["limits", "0,0", "48,36"]);
    typed(&mut s, &["base", "5,5"]);
    typed(&mut s, &["color", "red"]);
    typed(&mut s, &["lweight", "0.5"]);
    typed(&mut s, &["getvar", "ltscale"]);
    assert!(s.log.iter().rev().take(3).any(|l| l.contains("2.5")), "{:?}", s.log.last());
    typed(&mut s, &["find", "nothing-here"]);

    s.execute("layer", &json!({"action": "new", "name": "A"})).unwrap();
    s.execute("line", &json!({"points": [[0, 0], [10, 0]]})).unwrap();
    let l = s.doc().unwrap().model.iter().find(|e| matches!(e.kind, EntityKind::Line(_))).unwrap().handle;
    s.cmdline("breakatpoint").unwrap();
    s.input(Input::Pick(vec![l])).unwrap();
    typed(&mut s, &["5,0"]);
    let lines = s.doc().unwrap().model.iter().filter(|e| matches!(e.kind, EntityKind::Line(_))).count();
    assert_eq!(lines, 2);
}
