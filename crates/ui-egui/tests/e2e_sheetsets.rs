//! End to end: sheet sets through the control channel — a set made, drawings added, published as
//! one PDF; and the Sheet Set Manager palette opened from its menu shows the sheets.

mod e2e_common;

use e2e_common::{E2e, folder};
use serde_json::json;

#[test]
fn a_sheet_set_is_made_published_and_shown() {
    let mut e = E2e::new();
    let dir = folder("sheetsets");
    e.typed("rectang 0,0 100,60");
    e.typed("circle 50,30 10");
    let a = dir.join("A-101.dwg");
    e.exec("saveas", json!({ "path": a }));
    let set = dir.join("Project A.sheetset.json");
    e.exec("sheetset", json!({ "set": set, "action": "new", "name": "Project A" }));
    let r = e.exec("sheetset", json!({ "set": set, "action": "add", "drawing": a, "number": "A-101", "title": "Plan" }));
    assert!(r["sheets"].as_array().is_some_and(|s| !s.is_empty()), "{r}");
    let book = dir.join("Project A.pdf");
    let p = e.exec("sheetset", json!({ "set": set, "action": "publish", "path": book }));
    assert!(p["pages"].as_u64().unwrap_or(0) >= 1, "{p}");
    assert!(std::fs::read(&book).unwrap().starts_with(b"%PDF"));
    // The palette from its menu.
    e.menu("sheetset");
    assert_eq!(e.dialog().as_deref(), Some("sheetset"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// File dialogs that answer from a list, keeping what they were asked to suggest.
type Asked = std::rc::Rc<std::cell::RefCell<Vec<String>>>;

fn dialogs(e: &mut E2e, saves: Vec<std::path::PathBuf>, opens: Vec<std::path::PathBuf>) -> Asked {
    let asked: Asked = Default::default();
    let saves = std::rc::Rc::new(std::cell::RefCell::new(saves));
    let opens = std::rc::Rc::new(std::cell::RefCell::new(opens));
    let a = asked.clone();
    e.app.services.pick_save = Some(Box::new(move |name: &str| {
        a.borrow_mut().push(name.to_string());
        let mut s = saves.borrow_mut();
        (!s.is_empty()).then(|| s.remove(0).to_string_lossy().into_owned())
    }));
    e.app.services.pick_open = Some(Box::new(move || {
        let mut o = opens.borrow_mut();
        (!o.is_empty()).then(|| o.remove(0).to_string_lossy().into_owned())
    }));
    asked
}

#[test]
fn the_manager_makes_a_sheetset_json_publishes_next_to_it_and_warns_about_blank_sheets() {
    let mut e = E2e::new();
    let dir = folder("sheetset-manager");
    // A plan whose layouts were never opened, and a drawing with nothing in it.
    e.typed("rectang 0,0 480,360");
    let plan = dir.join("Plan.dwg");
    e.exec("saveas", json!({ "path": plan }));
    let mut blank = cadaddy_engine::doc::Drawing::new_imperial();
    blank.layouts.truncate(1);
    let empty = dir.join("Empty.dxf");
    std::fs::write(&empty, cadaddy_io::write(&blank, "Empty.dxf").unwrap()).unwrap();
    // Windows put the drawing type's `.cdy` on the typed name.
    let asked = dialogs(&mut e, vec![dir.join("Riverside Annex.cdy"), dir.join("Riverside Annex.pdf")], vec![plan.clone(), empty.clone()]);
    e.menu("sheetset");
    assert!(e.click("New Sheet Set..."));
    e.pump();
    let set = dir.join("Riverside Annex.sheetset.json");
    assert_eq!(e.app.ui.sheet_set.as_deref(), Some(set.to_string_lossy().as_ref()));
    assert!(set.exists() && !dir.join("Riverside Annex.cdy").exists());
    assert_eq!(asked.borrow().first().map(String::as_str), Some("Sheets.sheetset.json"));
    let listed = e.exec("sheetset", json!({ "set": set }));
    assert_eq!(listed["name"], "Riverside Annex", "the set's name, without an extension");
    assert!(e.click("Add Drawing..."));
    e.pump();
    assert!(e.click("Add Drawing..."));
    e.pump();
    let listed = e.exec("sheetset", json!({ "set": set }));
    assert_eq!(listed["sheets"].as_array().map_or(0, Vec::len), 3, "{listed}");
    // Publish suggests the set's own folder (the last dialog was in another).
    assert!(e.click("Publish to PDF..."));
    e.pump();
    let suggested = asked.borrow().get(1).cloned().unwrap_or_default();
    assert_eq!(std::path::Path::new(&suggested), std::path::absolute(dir.join("Riverside Annex.pdf")).unwrap());
    assert!(std::fs::read(dir.join("Riverside Annex.pdf")).unwrap().starts_with(b"%PDF"));
    // The empty drawing's sheet is warned about, on the command line before the result and in
    // the manager; the plan's layouts were set up and aren't.
    let h = e.history();
    let warn = h.iter().position(|l| l.starts_with("Warning:") && l.contains("Empty.dxf"));
    let done = h.iter().position(|l| l.starts_with("Published 3 sheets"));
    assert!(warn.is_some() && done.is_some() && warn < done, "{h:?}");
    assert_eq!(h.iter().filter(|l| l.starts_with("Warning:")).count(), 1, "{h:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn clicking_a_new_drawings_layout1_shows_what_was_drawn() {
    let mut e = E2e::new();
    e.exec("new", json!({}));
    e.typed("rectang 0,0 480,360");
    assert!(e.click("Layout1"), "the layout tab");
    e.pump();
    let d = e.app.session.doc().unwrap();
    let vps: Vec<_> = d
        .layout("Layout1")
        .unwrap()
        .entities
        .iter()
        .filter_map(|x| if let cadaddy_engine::doc::EntityKind::Viewport(v) = &x.kind { Some(v.clone()) } else { None })
        .filter(|v| v.id != 1)
        .collect();
    assert_eq!(vps.len(), 1);
    let v = &vps[0];
    let half_h = v.view_height / 2.0;
    let half_w = half_h * v.width / v.height;
    assert!(v.view_center.x - half_w <= 0.0 && v.view_center.x + half_w >= 480.0, "{v:?}");
    assert!(v.view_center.y - half_h <= 0.0 && v.view_center.y + half_h >= 360.0, "{v:?}");
}
