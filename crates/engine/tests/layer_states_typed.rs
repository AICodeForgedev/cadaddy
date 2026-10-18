//! -LAYER stAte typed at the command line: save, list, restore, rename and delete layer states.

use cadaddy_engine::Session;
use serde_json::json;

fn type_lines(s: &mut Session, lines: &[&str]) {
    for l in lines {
        s.cmdline(l).unwrap();
    }
}

fn layer_on(s: &Session, name: &str) -> bool {
    s.doc().unwrap().layer(name).unwrap().on
}

fn states(s: &Session) -> Vec<String> {
    s.doc().unwrap().layer_states.iter().map(|st| st.name.clone()).collect()
}

#[test]
fn dash_layer_state_saves_restores_renames_and_deletes() {
    let mut s = Session::new();
    s.execute("layer.new", &json!({ "name": "E-LITE" })).unwrap();
    // Save with E-LITE on, turn it off, restore (typed in another case): it is on again.
    type_lines(&mut s, &["-layer", "a", "s", "LightingOn"]);
    assert_eq!(states(&s), ["LightingOn"]);
    assert!(s.prompt_text().contains("[?/Save/Restore/Delete/Name]"), "{}", s.prompt_text());
    type_lines(&mut s, &["", "off", "E-LITE"]);
    assert!(!layer_on(&s, "E-LITE"));
    type_lines(&mut s, &["a", "r", "lightingon"]);
    assert!(layer_on(&s, "E-LITE"));
    // ? lists; an unknown state is reported and the state prompt comes back.
    type_lines(&mut s, &["?", "r", "nope"]);
    assert!(s.log.iter().any(|l| l.contains("Saved layer states: LightingOn")), "{:?}", s.log);
    assert!(s.log.iter().any(|l| l.contains("Cannot find layer state \"nope\"")), "{:?}", s.log);
    assert!(s.prompt_text().contains("[?/Save/Restore/Delete/Name]"), "{}", s.prompt_text());
    type_lines(&mut s, &["n", "LIGHTINGON", "Power"]);
    assert_eq!(states(&s), ["Power"]);
    type_lines(&mut s, &["d", "power", "", ""]);
    assert!(states(&s).is_empty());
    assert!(s.running.is_none(), "{}", s.prompt_text());
}
