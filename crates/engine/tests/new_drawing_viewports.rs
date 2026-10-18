//! A new drawing's layouts each start with one viewport (AutoCAD's default template), and the new
//! drawing isn't modified by it.

use cadaddy_doc::EntityKind;
use cadaddy_engine::Session;
use serde_json::json;

#[test]
fn new_drawings_have_a_viewport_on_each_layout() {
    let mut s = Session::empty();
    s.execute("new", &json!({})).unwrap();
    let st = s.state().unwrap();
    assert!(!st.is_dirty());
    for l in &st.doc.layouts {
        let vps = l.entities.iter().filter(|e| matches!(e.kind, EntityKind::Viewport(_))).count();
        assert_eq!(vps, 1, "{}", l.name);
    }
    // Undo has nothing to take back.
    assert!(st.undo.is_empty());
}
