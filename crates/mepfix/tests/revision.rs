//! Made-up revisions (Q270): the changes are made and known, only the sheets they touch change,
//! and the rest of the set is reissued byte for byte.

use cadaddy_mepfix::revision::{RevisionPlan, revise};

#[test]
fn a_revision_changes_only_the_sheets_it_lists() {
    let plan = RevisionPlan::default();
    let rev = revise(7, &plan).unwrap();
    let kinds: Vec<&str> = rev.changes.iter().map(|c| c.kind.as_str()).collect();
    assert_eq!(kinds.iter().filter(|k| **k == "resize").count(), 2, "{kinds:?}");
    assert_eq!(kinds.iter().filter(|k| **k == "raise").count(), 1, "{kinds:?}");
    assert!(kinds.contains(&"removed_terminal"), "{kinds:?}");
    for c in &rev.changes {
        assert_ne!(c.from, c.to, "{c:?}");
        assert!(c.at_in != [0.0, 0.0], "placed: {c:?}");
        assert_eq!([c.at_in[0] - c.was_at_in[0], c.at_in[1] - c.was_at_in[1]], plan.shift_in, "shifted with its sheet");
    }
    // The resized ducts are wider in the revised truth and their tags say so.
    for c in rev.changes.iter().filter(|c| c.kind == "resize") {
        let old = rev.issued.truth.segment(&c.item).unwrap();
        let new = rev.revised.truth.segment(&c.item).unwrap();
        assert_eq!(new.size_in[0], old.size_in[0] + 4.0);
        assert!(new.tag.as_ref().unwrap().text[0].contains(&c.to), "{:?}", new.tag);
    }
    let changed = rev.changed_sheets();
    assert!(!changed.is_empty());
    for sh in &rev.issued.truth.sheets {
        let a = rev.issued.file(&sh.dxf).unwrap();
        let b = rev.revised.file(&sh.dxf).unwrap();
        if changed.contains(&sh.number) {
            assert_ne!(a, b, "{} changed", sh.number);
        } else {
            assert_eq!(a, b, "{} is reissued as it was", sh.number);
        }
    }
    // The same plan, the same revision.
    let again = revise(7, &plan).unwrap();
    assert_eq!(again.changes, rev.changes);
    assert!(revise(7, &RevisionPlan { shift_in: [f64::NAN, 0.0], ..plan }).is_err());
}
