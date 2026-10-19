//! The geometry rows the Properties palette and Quick Properties show for several objects of one
//! type: each value as the objects share it, or *VARIES*; an edit sets that one value on every one
//! of them (`properties.set` with the field's key and the type, so other types in the selection
//! stay as they are). Coordinates go one at a time (`centerX`, `startY`…) so each object keeps its
//! other one.

use cadaddy_doc::{Entity, EntityKind, Handle};

/// One row: a property of objects of one type.
pub struct Field {
    pub label: &'static str,
    /// The `properties.set` key; [`Field::edit`] false: shown only.
    pub key: &'static str,
    pub edit: bool,
    /// A length (shown in the drawing's units) rather than an angle or factor.
    pub length: bool,
    pub get: fn(&EntityKind) -> Option<f64>,
}

const fn f(label: &'static str, key: &'static str, length: bool, get: fn(&EntityKind) -> Option<f64>) -> Field {
    Field { label, key, edit: true, length, get }
}

/// The fields of objects of type `ty` ([`EntityKind::type_name`]).
pub fn fields(ty: &str) -> Vec<Field> {
    match ty {
        "Line" => vec![
            f("Start X", "startX", true, |k| if let EntityKind::Line(l) = k { Some(l.a.x) } else { None }),
            f("Start Y", "startY", true, |k| if let EntityKind::Line(l) = k { Some(l.a.y) } else { None }),
            f("End X", "endX", true, |k| if let EntityKind::Line(l) = k { Some(l.b.x) } else { None }),
            f("End Y", "endY", true, |k| if let EntityKind::Line(l) = k { Some(l.b.y) } else { None }),
            f("Length", "length", true, |k| if let EntityKind::Line(l) = k { Some((l.b.xy() - l.a.xy()).len()) } else { None }),
            f("Angle", "angle", false, |k| if let EntityKind::Line(l) = k { Some((l.b.xy() - l.a.xy()).angle().to_degrees()) } else { None }),
        ],
        "Circle" => vec![
            f("Center X", "centerX", true, |k| if let EntityKind::Circle(c) = k { Some(c.center.x) } else { None }),
            f("Center Y", "centerY", true, |k| if let EntityKind::Circle(c) = k { Some(c.center.y) } else { None }),
            f("Radius", "radius", true, |k| if let EntityKind::Circle(c) = k { Some(c.radius) } else { None }),
            f("Diameter", "diameter", true, |k| if let EntityKind::Circle(c) = k { Some(c.radius * 2.0) } else { None }),
            f("Area", "area", false, |k| if let EntityKind::Circle(c) = k { Some(c.radius * c.radius * std::f64::consts::PI) } else { None }),
        ],
        "Arc" => vec![
            f("Center X", "centerX", true, |k| if let EntityKind::Arc(a) = k { Some(a.center.x) } else { None }),
            f("Center Y", "centerY", true, |k| if let EntityKind::Arc(a) = k { Some(a.center.y) } else { None }),
            f("Radius", "radius", true, |k| if let EntityKind::Arc(a) = k { Some(a.radius) } else { None }),
            f("Start angle", "startAngle", false, |k| if let EntityKind::Arc(a) = k { Some(a.start.to_degrees()) } else { None }),
            f("End angle", "endAngle", false, |k| if let EntityKind::Arc(a) = k { Some(a.end.to_degrees()) } else { None }),
        ],
        "Text" => vec![
            f("Height", "height", true, |k| if let EntityKind::Text(t) = k { Some(t.height) } else { None }),
            f("Rotation", "rotation", false, |k| if let EntityKind::Text(t) = k { Some(t.rotation.to_degrees()) } else { None }),
        ],
        "MText" => vec![
            f("Text height", "height", true, |k| if let EntityKind::MText(t) = k { Some(t.height) } else { None }),
            f("Rotation", "rotation", false, |k| if let EntityKind::MText(t) = k { Some(t.rotation.to_degrees()) } else { None }),
        ],
        "Insert" => vec![
            f("Rotation", "rotation", false, |k| if let EntityKind::Insert(i) = k { Some(i.rotation.to_degrees()) } else { None }),
            f("Scale", "scale", false, |k| if let EntityKind::Insert(i) = k { Some(i.scale.x) } else { None }),
        ],
        "LwPolyline" => vec![f("Global width", "width", true, |k| if let EntityKind::LwPolyline(p) = k { Some(p.const_width) } else { None })],
        _ => Vec::new(),
    }
    .into_iter()
    .map(|mut fd| {
        // Area follows the radius; shown only, as AutoCAD's multi-edit leaves it.
        if fd.key == "area" {
            fd.edit = false;
        }
        fd
    })
    .collect()
}

/// What a row shows for `ents`: the shared value, or `None` when they differ (*VARIES*).
pub fn shared(field: &Field, ents: &[&Entity]) -> Option<f64> {
    let mut vals = ents.iter().filter_map(|e| (field.get)(&e.kind));
    let first = vals.next()?;
    vals.all(|v| (v - first).abs() <= 1e-9 * first.abs().max(1.0)).then_some(first)
}

/// The object types in `ents` with how many of each, most first (ties in first-seen order).
pub fn types(ents: &[&Entity]) -> Vec<(&'static str, usize)> {
    let mut out: Vec<(&'static str, usize)> = Vec::new();
    for e in ents {
        let n = e.kind.type_name();
        match out.iter_mut().find(|(t, _)| *t == n) {
            Some((_, c)) => *c += 1,
            None => out.push((n, 1)),
        }
    }
    out.sort_by_key(|t| std::cmp::Reverse(t.1));
    out
}

/// The type a palette shows of the selection (`scope`: "palette" or "quick"), kept while the
/// selection stays the same.
fn filter_id(scope: &str) -> egui::Id {
    egui::Id::new(("multiprops_type", scope))
}

/// Show only the objects of type `ty` of `sel` in `scope` (`None`: all of them again).
pub fn set_filter(ctx: &egui::Context, scope: &str, sel: &[Handle], ty: Option<&str>) {
    ctx.data_mut(|d| match ty {
        Some(t) => {
            d.insert_temp(filter_id(scope), (sel.to_vec(), t.to_string()));
        }
        None => d.remove::<(Vec<Handle>, String)>(filter_id(scope)),
    });
}

/// The objects of `sel` that `scope` shows and edits, and the type it narrowed them to (when it
/// did and the selection is the one it was narrowed for).
pub fn filtered(ctx: &egui::Context, scope: &str, sel: &[Handle], d: &cadaddy_doc::Drawing) -> (Vec<Handle>, Option<String>) {
    let kept: Option<(Vec<Handle>, String)> = ctx.data(|m| m.get_temp(filter_id(scope)));
    match kept {
        Some((for_sel, ty)) if for_sel == sel => {
            let hs: Vec<Handle> = sel.iter().copied().filter(|h| d.entity(*h).is_some_and(|e| e.kind.type_name() == ty)).collect();
            if hs.is_empty() { (sel.to_vec(), None) } else { (hs, Some(ty)) }
        }
        _ => (sel.to_vec(), None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_doc::Circle;
    use cadaddy_geom::Vec3;

    fn circle(h: u64, x: f64, r: f64) -> Entity {
        Entity::new(Handle(h), EntityKind::Circle(Circle { center: Vec3 { x, y: 0.0, z: 0.0 }, radius: r }))
    }

    #[test]
    fn shared_values_and_varies() {
        let (a, b) = (circle(1, 0.0, 5.0), circle(2, 10.0, 5.0));
        let ents = vec![&a, &b];
        let fs = fields("Circle");
        let get = |k: &str| fs.iter().find(|f| f.key == k).and_then(|f| shared(f, &ents));
        assert_eq!(get("radius"), Some(5.0));
        assert_eq!(get("centerX"), None, "*VARIES*");
        assert!(fs.iter().any(|f| f.key == "area" && !f.edit));
        assert_eq!(types(&ents), vec![("Circle", 2)]);
        assert!(fields("Hatch").is_empty());
    }

    #[test]
    fn panels_narrow_a_mixed_selection_and_quick_lists_are_kept_per_type() {
        use crate::CadApp;
        use serde_json::json;
        let mut app = CadApp::new(cadaddy_engine::Session::new(), crate::Services::default());
        let c = app.run("circle", json!({"center": [0, 0], "radius": 2})).unwrap()["handle"].as_str().unwrap().to_string();
        let l = app.run("line", json!({"points": [[0, 0], [5, 0]]})).unwrap()["handles"][0].as_str().unwrap().to_string();
        app.run("select", json!({"handles": [c, l]})).unwrap();
        let sel = app.session.selection();
        let ctx = egui::Context::default();
        // Narrowed to circles: the palettes show (and change) the circle only; the selection stays.
        set_filter(&ctx, "quick", &sel, Some("Circle"));
        let d = app.session.doc().unwrap();
        let (shown, ty) = filtered(&ctx, "quick", &sel, d);
        assert_eq!((shown.len(), ty.as_deref()), (1, Some("Circle")));
        assert_eq!(filtered(&ctx, "palette", &sel, d).0.len(), 2, "each panel has its own");
        // A different selection forgets it.
        assert_eq!(filtered(&ctx, "quick", &sel[..1], d).1, None);
        // Both panels draw a mixed, narrowed selection without a crash.
        app.session.settings.qpmode = true;
        for _ in 0..2 {
            ctx.run_ui(egui::RawInput::default(), |ui| {
                crate::quick::quick_properties(&mut app, ui.ctx());
                crate::palettes::properties_section(&mut app, ui);
            })
            .textures_delta
            .clear();
        }
        assert_eq!(app.session.selection().len(), 2);
        // Quick Properties lists: set, read, refused, put back.
        let r = crate::quick::lists_command(&mut app, &json!({"type": "Circle", "fields": ["layer", "radius"]})).unwrap();
        assert_eq!(r["lists"]["Circle"], json!(["layer", "radius"]));
        assert!(r["available"].as_array().unwrap().iter().any(|k| k == "centerX"));
        assert!(crate::quick::lists_command(&mut app, &json!({"type": "Circle", "fields": ["nope"]})).is_err());
        assert!(crate::quick::lists_command(&mut app, &json!({"type": "Circle", "fields": 3})).is_err());
        ctx.run_ui(egui::RawInput::default(), |ui| crate::quick::quick_properties(&mut app, ui.ctx())).textures_delta.clear();
        crate::quick::lists_command(&mut app, &json!({"type": "Circle", "fields": null})).unwrap();
        assert!(app.ui.quick_properties.is_empty());
        // Through the UI command too (agents, scripts).
        assert!(app.run("ui.quickprops", json!({"type": "Line", "fields": ["length"]})).is_ok());
        assert_eq!(app.ui.quick_properties.get("Line"), Some(&vec!["length".to_string()]));
    }
}
