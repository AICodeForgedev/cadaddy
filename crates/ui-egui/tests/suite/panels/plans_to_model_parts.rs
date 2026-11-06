//! Edit and Remove in the Plans to Model palette, for one part of every kind of spec section:
//! Edit shows the part's own JSON, Apply unchanged changes nothing (and says so), Remove takes
//! out exactly that part (or isn't offered); one ceiling or soffit type removed or edited leaves
//! the others as they were.

use cadaddy_engine::Session;
use cadaddy_ui_egui::control::{ControlRequest, Outcome, handle};
use cadaddy_ui_egui::{CadApp, Services};
use egui::{Key, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{Queryable, by};
use serde_json::{Value, json};

const UNCHANGED: &str = "Nothing changed: the building spec is the same as before.";

fn call(app: &mut CadApp, method: &str, params: Value) -> Value {
    let (req, _reply) = ControlRequest::new(method, params);
    match handle(app, &egui::Context::default(), &req) {
        Outcome::Done(v) => {
            assert_eq!(v["ok"], json!(true), "{method}: {v}");
            v["result"].clone()
        }
        Outcome::Screenshot { .. } => Value::Null,
    }
}

fn exec(app: &mut CadApp, command: &str, params: Value) -> Value {
    call(app, "engine.execute", json!({ "command": command, "params": params }))
}

fn harness() -> Harness<'static, CadApp> {
    let mut app = CadApp::new(Session::new(), Services::default());
    call(&mut app, "ui.menu.invoke", json!({"command": "ui.panel.planstomodel"}));
    app.ui.in_window_menu = false;
    let mut h = Harness::builder().with_size(vec2(1500.0, 1000.0)).build_ui_state(
        |ui, app: &mut CadApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    h
}

/// Put `sections` in the drawing as its whole spec, with one level.
fn set(h: &mut Harness<'static, CadApp>, sections: Value) {
    let mut spec = json!({"levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9}]});
    for (k, v) in sections.as_object().into_iter().flatten() {
        spec[k] = v.clone();
    }
    exec(h.state_mut(), "bspec.set", json!({"replace": true, "spec": spec}));
    h.run_steps(3);
}

fn spec(h: &mut Harness<'static, CadApp>) -> Value {
    exec(h.state_mut(), "bspec.get", json!({}))["spec"].clone()
}

fn status(h: &Harness<'static, CadApp>) -> Option<String> {
    h.state().status.as_ref().map(|(s, _)| s.clone())
}

/// The top of the row whose text starts with `text`.
fn row(h: &Harness<'static, CadApp>, text: &str) -> f32 {
    let starts = |s: Option<String>| s.is_some_and(|l| l.starts_with(text));
    h.get_all_by(|n| starts(n.label()) || starts(n.value())).map(|n| n.rect().top()).reduce(f32::min).unwrap_or_else(|| panic!("no row {text}"))
}

/// The `button` nearest under the row starting with `text`, if it is that row's (above the next
/// part's text).
fn button<'a>(h: &'a Harness<'static, CadApp>, text: &str, button: &'a str) -> Option<egui_kittest::Node<'a>> {
    let top = row(h, text);
    let edit = h.get_all_by_label("Edit").map(|e| e.rect().top()).filter(|t| *t > top).fold(f32::INFINITY, f32::min);
    h.get_all_by_label(button)
        .filter(|b| b.rect().top() > top && b.rect().top() <= edit + 1.0)
        .min_by(|a, b| a.rect().top().total_cmp(&b.rect().top()))
}

/// Scroll the row's `button` into view and click it.
fn press(h: &mut Harness<'static, CadApp>, text: &str, label: &str) {
    button(h, text, label).unwrap_or_else(|| panic!("no {label} for {text}")).scroll_to_me();
    // The scroll is animated.
    h.run_steps(12);
    button(h, text, label).unwrap_or_else(|| panic!("no {label} for {text}")).click();
    h.run_steps(3);
}

/// Scroll the editor's Apply into view and click it.
fn press_apply(h: &mut Harness<'static, CadApp>) {
    h.get_by_label("Apply").scroll_to_me();
    // The scroll is animated.
    h.run_steps(12);
    h.get_by_label("Apply").click();
    h.run_steps(4);
}

fn editor() -> egui_kittest::kittest::By<'static> {
    by().role(egui::accesskit::Role::MultilineTextInput)
}

/// Edit the part: the editor shows `want`; Apply unchanged leaves the spec as it was and says so.
fn edit_unchanged(h: &mut Harness<'static, CadApp>, text: &str, want: &Value) {
    let before = spec(h);
    press(h, text, "Edit");
    let shown: Value = serde_json::from_str(&h.get(editor()).value().unwrap_or_default()).unwrap_or(Value::Null);
    assert_eq!(&shown, want, "{text}");
    assert!(!shown.is_null(), "{text}");
    h.state_mut().status = None;
    press_apply(h);
    assert_eq!(spec(h), before, "{text}");
    assert!(h.query(editor()).is_none(), "{text}: the editor closes");
    assert_eq!(status(h).as_deref(), Some(UNCHANGED), "{text}");
}

/// Remove the part: the spec afterwards is `want`.
fn remove(h: &mut Harness<'static, CadApp>, text: &str, want: &Value) {
    h.state_mut().status = None;
    press(h, text, "Remove");
    assert_eq!(&spec(h), want, "{text}");
    assert_eq!(status(h), None, "{text}");
}

/// `spec` without the list item at `pointer`/`i`.
fn without(spec: &Value, pointer: &str, i: &[usize]) -> Value {
    let mut s = spec.clone();
    let list = s.pointer_mut(pointer).and_then(Value::as_array_mut).unwrap_or_else(|| panic!("{pointer}"));
    for at in i.iter().rev() {
        list.remove(*at);
    }
    s
}

fn at(spec: &Value, pointer: &str) -> Value {
    spec.pointer(pointer).cloned().unwrap_or_else(|| panic!("{pointer} in {spec}"))
}

fn square(x: f64) -> Value {
    json!([[x, 0], [x + 10.0, 0], [x + 10.0, 10], [x, 10]])
}

#[test]
fn edit_and_remove_work_on_one_part_of_every_section() {
    let mut h = harness();
    // (sections, the part's row text, where its JSON is, the list items Remove takes out).
    let cases: Vec<(Value, &str, &str, Vec<usize>)> = vec![
        (json!({}), "L1 = ", "/levels/0", vec![0]),
        (json!({"layers": [{"layer": "A-WALL", "role": "wall"}, {"layer": "A-DOOR", "role": "door"}]}), "A-DOOR = doors", "/layers/1", vec![1]),
        (json!({"wall_types": [{"name": "WT1", "thickness_in": 5}, {"name": "WT2", "thickness_in": 8}]}), "WT1: ", "/wall_types/0", vec![0]),
        (json!({"rooms": [{"name": "Office"}, {"name": "Storage"}]}), "Storage: ", "/rooms/1", vec![1]),
        (
            json!({"families": [{"name": "D1", "kind": "door", "block": "DOOR"}, {"name": "W1", "kind": "window", "block": "WIN"}]}),
            "W1: ",
            "/families/1",
            vec![1],
        ),
        (
            json!({"stairs": [
                {"name": "S1", "level": "L1", "runs": [{"from": [0, 0], "to": [0, 10]}]},
                {"name": "S2", "level": "L1", "runs": [{"from": [20, 0], "to": [20, 10]}]}
            ]}),
            "S1: ",
            "/stairs/0",
            vec![0],
        ),
        (json!({"ceiling_types": [{"name": "C1"}, {"name": "C2", "kind": "grid"}]}), "C2: ", "/ceiling_types/1", vec![1]),
        (json!({"soffit_types": [{"name": "SF1"}, {"name": "SF2", "hangs_from": "structure"}]}), "SF1: ", "/soffit_types/0", vec![0]),
        (
            json!({"mep": {"systems": [
                {"name": "SA", "layer": "M-SUPP", "kind": "duct", "size_in": [12, 8], "bottom_ft": 9},
                {"name": "RA", "layer": "M-RETN", "kind": "duct", "size_in": [12, 8], "bottom_ft": 9}
            ]}}),
            "RA: ",
            "/mep/systems/1",
            vec![1],
        ),
        (
            json!({"mep": {"equipment": [
                {"tag": "AHU-1", "kind": "air_handler", "level": "L1", "height_ft": 4},
                {"tag": "AHU-2", "kind": "air_handler", "level": "L1", "height_ft": 5}
            ]}}),
            "AHU-2: ",
            "/mep/equipment/1",
            vec![1],
        ),
    ];
    for (sections, text, pointer, gone) in cases {
        set(&mut h, sections);
        let s = spec(&mut h);
        edit_unchanged(&mut h, text, &at(&s, pointer));
        let list = &pointer[..pointer.rfind('/').unwrap_or(0)];
        remove(&mut h, text, &without(&s, list, &gone));
    }

    // Exterior: a facade, a roof, a cricket and rooftop equipment, each by its place in its list.
    let exterior = json!({"exterior": {
        "facades": [{"side": "north"}, {"side": "south", "bands": [{"kind": "glass", "from_ft": 3, "to_ft": 8}]}],
        "roofs": [{"name": "R1", "level": "L1", "outline": square(0.0)}, {"name": "R2", "level": "L1", "outline": square(20.0), "parapet_ft": 3}],
        "crickets": [{"level": "L1", "outline": square(0.0), "height_ft": 1}, {"level": "L1", "outline": square(20.0), "height_ft": 2}],
        "equipment": [
            {"name": "RTU-1", "level": "L1", "outline": square(0.0), "height_ft": 4},
            {"name": "RTU-2", "level": "L1", "outline": square(20.0), "height_ft": 5}
        ]
    }});
    for (text, list, i) in [
        ("South facade", "/exterior/facades", 1),
        ("R2: roof", "/exterior/roofs", 1),
        ("Cricket 1 ", "/exterior/crickets", 0),
        ("RTU-1: rooftop", "/exterior/equipment", 0),
    ] {
        set(&mut h, exterior.clone());
        let s = spec(&mut h);
        edit_unchanged(&mut h, text, &at(&s, &format!("{list}/{i}")));
        remove(&mut h, text, &without(&s, list, &[i]));
    }

    // RCP: a ceiling and a soffit by place; the ceiling items as one part.
    let rcp = json!({"rcp": {
        "ceilings": [{"level": "L1", "outline": square(0.0), "height_ft": 9}, {"level": "L1", "outline": square(20.0), "height_ft": 8}],
        "soffits": [{"level": "L1", "outline": square(0.0), "height_ft": 8}, {"level": "L1", "outline": square(20.0), "height_ft": 7}],
        "items": [
            {"kind": "fixture", "block": "LT", "level": "L1", "at": [1, 1], "size_in": [24, 24]},
            {"kind": "diffuser", "block": "SD", "level": "L1", "at": [5, 5], "size_in": [24, 24]}
        ]
    }});
    for (text, list, i) in [("Ceiling 2 ", "/rcp/ceilings", 1), ("Soffit 1 ", "/rcp/soffits", 0)] {
        set(&mut h, rcp.clone());
        let s = spec(&mut h);
        edit_unchanged(&mut h, text, &at(&s, &format!("{list}/{i}")));
        remove(&mut h, text, &without(&s, list, &[i]));
    }
    set(&mut h, rcp);
    let s = spec(&mut h);
    edit_unchanged(&mut h, "In the ceilings: ", &at(&s, "/rcp/items"));
    let mut want = s.clone();
    want["rcp"].as_object_mut().map(|o| o.remove("items"));
    remove(&mut h, "In the ceilings: ", &want);

    // Columns by level and schedule rows by kind: the part is all of them, shown as a list,
    // even when the list mixes them with others.
    set(
        &mut h,
        json!({"columns": [
            {"level": "L1", "grid": "A-1", "at": [0, 0], "size_in": [12, 12]},
            {"level": "L2", "grid": "A-1", "at": [0, 0], "size_in": [12, 12]},
            {"level": "L1", "grid": "B-1", "at": [20, 0], "size_in": [12, 12]}
        ]}),
    );
    let s = spec(&mut h);
    edit_unchanged(&mut h, "2 columns on L1", &json!([at(&s, "/columns/0"), at(&s, "/columns/2")]));
    remove(&mut h, "2 columns on L1", &without(&s, "/columns", &[0, 2]));
    set(&mut h, json!({"schedule": {"rows": [{"kind": "door", "mark": "101"}, {"kind": "window", "mark": "A"}, {"kind": "door", "mark": "102"}]}}));
    let s = spec(&mut h);
    edit_unchanged(&mut h, "Door schedule: ", &json!([at(&s, "/schedule/rows/0"), at(&s, "/schedule/rows/2")]));
    remove(&mut h, "Door schedule: ", &without(&s, "/schedule/rows", &[0, 2]));

    // Whole values: walls, materials and site are removed; the openings can't be.
    set(
        &mut h,
        json!({
            "walls": [{"level": "L1", "a": [0, 0], "b": [10, 0], "wall_type": "WT1"}, {"level": "L1", "a": [0, 0], "b": [0, 10], "wall_type": "WT1"}],
            "materials": {"wall": "Brick"},
            "site": {"elevation_ft": 0, "margin_ft": 20}
        }),
    );
    let s = spec(&mut h);
    edit_unchanged(&mut h, "2 walls: ", &at(&s, "/walls"));
    let mut want = s.clone();
    want.as_object_mut().map(|o| o.remove("walls"));
    remove(&mut h, "2 walls: ", &want);
    edit_unchanged(&mut h, "Materials: ", &at(&s, "/materials"));
    want["materials"] = json!({});
    remove(&mut h, "Materials: ", &want);
    edit_unchanged(&mut h, "Ground at ", &at(&s, "/site"));
    want["site"] = Value::Null;
    remove(&mut h, "Ground at ", &want);
    edit_unchanged(&mut h, "Openings in walls: ", &at(&s, "/openings"));
    assert!(button(&h, "Openings in walls: ", "Remove").is_none(), "the openings are never removed");

    // A room with only a number can't be picked out by name: no Edit or Remove for it.
    set(&mut h, json!({"rooms": [{"name": "Office"}, {"name": "", "number": "102"}]}));
    assert!(button(&h, "Office: ", "Edit").is_some());
    let numbered = row(&h, "102: ");
    let next = row(&h, "Openings in walls: ");
    assert!(h.get_all_by_label("Edit").all(|e| e.rect().top() < numbered || e.rect().top() > next), "no Edit under the numbered room");
    assert!(h.get_all_by_label("Remove").all(|e| e.rect().top() < numbered), "no Remove under the numbered room");
}

/// Type `text` over the open editor's JSON and apply it.
fn apply(h: &mut Harness<'static, CadApp>, text: &str) {
    h.get(editor()).focus();
    h.run_steps(2);
    h.key_press_modifiers(egui::Modifiers::COMMAND, Key::A);
    h.run_steps(1);
    h.get(editor()).type_text(text);
    h.run_steps(2);
    press_apply(h);
}

#[test]
fn one_ceiling_or_soffit_type_removed_or_edited_leaves_the_others() {
    let mut h = harness();
    let ceilings = json!({"ceiling_types": [
        {"name": "C1", "description": "Gypsum board ceiling", "kind": "gypsum", "fire_rating": "1 HR"},
        {"name": "C2", "description": "Lay-in tile", "kind": "grid", "grid_in": [24, 24]},
        {"name": "C3", "description": "Exposed structure", "kind": "open"}
    ]});
    let soffits = json!({"soffit_types": [
        {"name": "S1", "description": "Gypsum soffit"},
        {"name": "S2", "description": "Hung soffit", "hangs_from": "structure", "above_ceiling_in": 6},
        {"name": "S3", "description": "Bulkhead"}
    ]});
    for (sections, list, second) in [(ceilings, "ceiling_types", "C2: "), (soffits, "soffit_types", "S2: ")] {
        set(&mut h, sections.clone());
        let types = spec(&mut h)[list].clone();
        assert_eq!(types.as_array().map(Vec::len), Some(3));
        // Remove the second: exactly the first and third are left, as they were.
        h.state_mut().status = None;
        press(&mut h, second, "Remove");
        assert_eq!(spec(&mut h)[list], json!([types[0], types[2]]), "{list}");
        assert_eq!(status(&h), None);
        // Edit the first's description: the others are untouched.
        set(&mut h, sections);
        let first = types[0]["name"].as_str().unwrap_or_default().to_string();
        press(&mut h, &format!("{first}: "), "Edit");
        let mut edited = types[0].clone();
        edited["description"] = json!("Changed by a reviewer");
        apply(&mut h, &edited.to_string());
        assert_eq!(spec(&mut h)[list], json!([edited, types[1], types[2]]), "{list}");
        assert_eq!(status(&h), None, "a real change isn't reported as none");
    }
}
