//! What a drawing keeps of how it is looked at (Q6575).
//!
//! Some of how the built model is looked at is kept in the drawing, so it comes back when the
//! drawing is opened again: the BIM view's look (on or off, ghosting, the level and the view
//! shown, trade colours and trade switches, tree branches hidden, derived work apart, the
//! presentation look). Changing it is not an edit, as zoom and pan are not: a drawing with
//! nothing unsaved still has nothing unsaved after it (no `*` in the title, no close prompt, an
//! agent's quit not held for the person), and undoing or redoing such a change keeps it so. The
//! look is still written with the drawing at its next save. A drawing with unsaved edits keeps
//! them unsaved whatever the view does.
//!
//! The test is on what changed, not on which command ran: any command (or plug-in, or panel)
//! whose only change to the drawing is these entries keeps a saved drawing saved. Rules a person
//! writes into the drawing (the fidelity tiers, the views' rules) are settings and stay edits.

use cadaddy_doc::Drawing;
use serde_json::Value;

/// The building spec's entries that are only how the model is looked at.
pub(crate) const VIEW_SPEC_KEYS: &[&str] = &[crate::cmd::bimlook::KEY];

/// The building spec without its view entries; `None` when its text is not a JSON object (then
/// nothing about it can be told apart).
fn spec_without_view(d: &Drawing) -> Option<Value> {
    let mut v = match d.building_spec.as_deref() {
        None => Value::Object(serde_json::Map::new()),
        Some(t) => serde_json::from_str(t).ok()?,
    };
    let o = v.as_object_mut()?;
    for k in VIEW_SPEC_KEYS {
        o.remove(*k);
    }
    Some(v)
}

/// Whether `a` and `b` differ, and only in how the model is looked at ([`VIEW_SPEC_KEYS`]).
pub(crate) fn only_view_differs(a: &Drawing, b: &Drawing) -> bool {
    // The same spec: whatever differs is elsewhere.
    if a.building_spec.same(&b.building_spec) {
        return false;
    }
    match (spec_without_view(a), spec_without_view(b)) {
        (Some(x), Some(y)) if x == y => {}
        _ => return false,
    }
    // Everything else as it was (objects, tables, layouts, settings): `b` with `a`'s spec is `a`.
    // The copy shares `b`'s objects, so it costs little and compares quickly.
    let mut probe = b.clone();
    probe.building_spec = a.building_spec.clone();
    probe == *a
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_spec(d: &Drawing, spec: Option<&str>) -> Drawing {
        let mut d = d.clone();
        d.building_spec = spec.map(str::to_string).into();
        d
    }

    #[test]
    fn only_the_look_changing_is_a_view_change() {
        let base = with_spec(&Drawing::default(), Some(r#"{"version":1,"levels":[]}"#));
        let looked = with_spec(&base, Some(r#"{"levels":[],"version":1,"bim_view":{"ghost":true}}"#));
        assert!(only_view_differs(&base, &looked));
        assert!(only_view_differs(&looked, &base));
        // No spec at all and a spec of only a look.
        let none = with_spec(&base, None);
        assert!(only_view_differs(&none, &with_spec(&base, Some(r#"{"bim_view":{"off":true}}"#))));
    }

    #[test]
    fn anything_else_changing_is_an_edit() {
        let base = with_spec(&Drawing::default(), Some(r#"{"version":1}"#));
        // Another entry of the spec (rules a person sets).
        assert!(!only_view_differs(&base, &with_spec(&base, Some(r#"{"version":1,"fidelity":{"rules":[]},"bim_view":{"ghost":true}}"#))));
        // The look and an object.
        let mut drawn = with_spec(&base, Some(r#"{"version":1,"bim_view":{"ghost":true}}"#));
        let point = cadaddy_doc::EntityKind::Point(cadaddy_doc::Point { p: cadaddy_geom::Vec3::new(1.0, 2.0, 0.0), angle: 0.0 });
        drawn.add(&cadaddy_doc::Space::Model, cadaddy_doc::Common::default(), point).unwrap();
        assert!(!only_view_differs(&base, &drawn));
        // A spec that isn't JSON can't be told apart.
        assert!(!only_view_differs(&base, &with_spec(&base, Some("not json"))));
        // Nothing changed in the spec.
        assert!(!only_view_differs(&base, &base.clone()));
    }
}
