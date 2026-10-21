//! Sheet sets: create one from drawings, renumber, rename layouts for the sheet fields, and
//! publish the whole set to one PDF with a bookmark per sheet under its subset.

use std::path::{Path, PathBuf};

use cadaddy_engine::Session;
use cadaddy_engine::cmd::file::{IoHooks, set_io, set_pdf_merge};
use cadaddy_engine::doc::geom::Vec3;
use cadaddy_engine::doc::{Drawing, Entity, EntityKind, Layout, Line, Space};
use serde_json::{Value, json};

fn hooks() {
    set_io(IoHooks {
        read: |b, name| cadaddy_io::read(b, name).map_err(|e| e.to_string()),
        write: |d, name| cadaddy_io::write(d, name).map_err(|e| e.to_string()),
        plot: Some(|d, space, opts| cadaddy_io::plotter::plot(d, space, opts).map_err(|e| e.to_string())),
        read_pdf: None,
    });
    set_pdf_merge(|parts| cadaddy_io::pdf_merge::merge(parts).map_err(|e| e.to_string()));
}

fn folder(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cadaddy-sheetset-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A drawing with a line in each of `layouts`.
fn drawing(path: &Path, layouts: &[&str]) {
    let mut d = Drawing::new_metric();
    d.layouts = layouts.iter().enumerate().map(|(i, n)| Layout::new(n, i as u32 + 1)).collect();
    for n in layouts {
        let h = d.new_handle();
        let e = Entity::new(h, EntityKind::Line(Line { a: Vec3::new(10.0, 10.0, 0.0), b: Vec3::new(100.0, 50.0, 0.0) }));
        d.space_mut(&Space::Paper((*n).to_string())).unwrap().push(e);
    }
    std::fs::write(path, cadaddy_io::write(&d, &path.to_string_lossy()).unwrap()).unwrap();
}

fn numbers(r: &Value) -> Vec<String> {
    r["sheets"].as_array().unwrap().iter().map(|s| s["number"].as_str().unwrap().to_string()).collect()
}

#[test]
fn create_add_renumber_sync_and_publish() {
    hooks();
    let dir = folder("full");
    drawing(&dir.join("arch.dxf"), &["Plan", "Elevations"]);
    std::fs::create_dir_all(dir.join("struct")).unwrap();
    drawing(&dir.join("struct/frame.dxf"), &["Framing"]);
    let set = dir.join("job.sheetset.json");
    let mut s = Session::new();
    let ss = |s: &mut Session, p: Value| {
        let mut p = p;
        p["set"] = json!(set.to_string_lossy());
        s.execute("sheetset", &p)
    };
    let r = ss(&mut s, json!({ "action": "new", "description": "Test set" })).unwrap();
    assert_eq!(r["name"], "job");
    assert!(ss(&mut s, json!({ "action": "new" })).is_err(), "it exists");

    // Every layout of a drawing, then one layout of another in its own subset.
    ss(&mut s, json!({ "action": "add", "drawing": dir.join("arch.dxf").to_string_lossy(), "subset": "Architectural" })).unwrap();
    let r = ss(
        &mut s,
        json!({ "action": "add", "drawing": dir.join("struct/frame.dxf").to_string_lossy(), "layouts": ["framing"], "subset": "Structural", "title": "Roof framing" }),
    )
    .unwrap();
    let sheets = r["sheets"].as_array().unwrap();
    assert_eq!(sheets.len(), 3);
    assert_eq!(sheets[2]["drawing"], "struct/frame.dxf", "stored relative to the set");
    assert_eq!(sheets[2]["title"], "Roof framing");
    assert!(sheets.iter().all(|s| s["found"] == true));
    assert_eq!(r["subsets"], json!(["Architectural", "Structural"]));

    // Renumber, then move the framing sheet first and number it on its own.
    let r = ss(&mut s, json!({ "action": "renumber", "prefix": "A-", "digits": 3, "start": 101 })).unwrap();
    assert_eq!(numbers(&r), ["A-101", "A-102", "A-103"]);
    let r = ss(&mut s, json!({ "action": "set", "sheet": "A-103", "number": "S-1" })).unwrap();
    assert_eq!(numbers(&r), ["A-101", "A-102", "S-1"]);
    let r = ss(&mut s, json!({ "action": "move", "sheet": 2, "to": 0 })).unwrap();
    assert_eq!(numbers(&r), ["S-1", "A-101", "A-102"]);
    let r = ss(&mut s, json!({ "action": "move", "sheet": "S-1", "to": 9 })).unwrap();
    assert_eq!(numbers(&r), ["A-101", "A-102", "S-1"]);

    // Layouts take "number title", so the drawings' sheet fields show them.
    ss(&mut s, json!({ "action": "sync" })).unwrap();
    let arch = cadaddy_io::read(&std::fs::read(dir.join("arch.dxf")).unwrap(), "arch.dxf").unwrap();
    let mut names: Vec<String> = arch.layouts.iter().map(|l| l.name.clone()).collect();
    names.sort();
    assert_eq!(names, ["A-101 Plan", "A-102 Elevations"]);
    assert!(dir.join("arch.bak").exists(), "the previous version is kept");

    // One PDF, a page per sheet, bookmarked under the subsets.
    let out = dir.join("job.pdf");
    let r = ss(&mut s, json!({ "action": "publish", "path": out.to_string_lossy() })).unwrap();
    assert_eq!(r["pages"], 3);
    let marks = cadaddy_io::pdf_merge::bookmarks(&std::fs::read(&out).unwrap()).unwrap();
    let want: Vec<(String, usize, usize)> = vec![
        ("Architectural".into(), 0, 0),
        ("A-101 Plan".into(), 1, 1),
        ("A-102 Elevations".into(), 1, 2),
        ("Structural".into(), 0, 0),
        ("S-1 Roof framing".into(), 1, 3),
    ];
    assert_eq!(marks, want);

    // The set file is ours: JSON with its marker.
    let text = std::fs::read_to_string(&set).unwrap();
    assert!(text.contains("\"cadaddySheetSet\": 1"), "{text}");
}

#[test]
fn subsets_removal_and_refusals() {
    hooks();
    let dir = folder("misc");
    drawing(&dir.join("a.dxf"), &["L1", "L2"]);
    let set = dir.join("s.sheetset.json").to_string_lossy().into_owned();
    let mut s = Session::new();
    s.execute("sheetset", &json!({ "set": set, "action": "new", "name": "Small" })).unwrap();
    s.execute("sheetset", &json!({ "set": set, "action": "add", "drawings": [dir.join("a.dxf").to_string_lossy()], "subset": "One" })).unwrap();
    let r = s.execute("sheetset", &json!({ "set": set, "action": "subset", "rename": ["one", "First"] })).unwrap();
    assert_eq!(r["sheets"][0]["subset"], "First");
    let r = s.execute("sheetset", &json!({ "set": set, "action": "subset", "remove": "First" })).unwrap();
    assert_eq!((r["subsets"].as_array().unwrap().len(), r["sheets"][0]["subset"].clone()), (0, json!("")));
    let r = s.execute("sheetset", &json!({ "set": set, "action": "remove", "sheets": [0, "2"] })).unwrap();
    assert_eq!(r["sheets"].as_array().unwrap().len(), 0);
    // Publishing an empty set, AutoCAD's .dst files and bad input are refused.
    for p in [
        json!({ "set": set, "action": "publish", "path": dir.join("x.pdf").to_string_lossy() }),
        json!({ "set": dir.join("old.dst").to_string_lossy(), "action": "list" }),
        json!({ "set": set, "action": "add", "drawing": dir.join("missing.dxf").to_string_lossy() }),
        json!({ "set": set, "action": "add", "drawing": dir.join("a.dxf").to_string_lossy(), "layouts": ["Nope"] }),
        json!({ "set": set, "action": "set", "sheet": 5 }),
        json!({ "set": set, "action": "wobble" }),
        json!({ "action": "list" }),
        json!({ "set": dir.join("none.json").to_string_lossy(), "action": "list" }),
    ] {
        assert!(s.execute("sheetset", &p).is_err(), "{p}");
    }
    let e = s.execute("sheetset", &json!({ "set": dir.join("old.dst").to_string_lossy() })).unwrap_err();
    assert!(e.to_string().contains(".dst"), "{e}");
}

#[test]
fn a_new_set_is_a_sheetset_json_file_named_without_its_extension() {
    let dir = folder("names");
    let mut s = Session::new();
    // What a Save dialog may hand back: a drawing's `.cdy`, a bare `.json`, no extension.
    for (given, file) in [
        ("Riverside Annex.cdy", "Riverside Annex.sheetset.json"),
        ("Mill Yard.json", "Mill Yard.sheetset.json"),
        ("North Wing", "North Wing.sheetset.json"),
        ("Depot.sheetset.json", "Depot.sheetset.json"),
        ("Kiln.sheetset.json.cdy", "Kiln.sheetset.json"),
    ] {
        let r = s.execute("sheetset", &json!({ "set": dir.join(given).to_string_lossy(), "action": "new" })).unwrap();
        let want = dir.join(file);
        assert_eq!(r["set"], json!(want.to_string_lossy()), "{given}");
        assert!(want.exists(), "{given}");
        assert_eq!(r["name"], file.trim_end_matches(".sheetset.json"), "{given}");
        let r = s.execute("sheetset", &json!({ "set": want.to_string_lossy() })).unwrap();
        assert_eq!(r["name"], file.trim_end_matches(".sheetset.json"));
    }
    assert!(!dir.join("Riverside Annex.cdy").exists(), "never a .cdy");
    let names: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert!(names.iter().all(|n| n.ends_with(".sheetset.json")), "{names:?}");
    // A given name wins; an existing set isn't replaced unasked.
    let r = s.execute("sheetset", &json!({ "set": dir.join("Yard 2.cdy").to_string_lossy(), "action": "new", "name": "Yard Two" })).unwrap();
    assert_eq!(r["name"], "Yard Two");
    assert!(s.execute("sheetset", &json!({ "set": dir.join("Yard 2").to_string_lossy(), "action": "new" })).is_err());
}

/// A drawing whose model space holds a plan and whose layouts are empty (as a DWG made elsewhere
/// often has), or one with nothing drawn at all.
fn bare_drawing(path: &Path, with_plan: bool) {
    let mut d = Drawing::new_imperial();
    for i in 0..if with_plan { 60 } else { 0 } {
        let (x, y) = (f64::from(i * 37 % 480), f64::from(i * 53 % 360));
        let h = d.new_handle();
        d.space_mut(&Space::Model)
            .unwrap()
            .push(Entity::new(h, EntityKind::Line(Line { a: Vec3::new(x, 0.0, 0.0), b: Vec3::new(480.0 - y, y, 0.0) })));
    }
    assert!(d.layouts.iter().all(|l| l.entities.is_empty()));
    std::fs::write(path, cadaddy_io::write(&d, &path.to_string_lossy()).unwrap()).unwrap();
}

fn publish(s: &mut Session, set: &Path, out: &Path) -> Value {
    s.execute("sheetset", &json!({ "set": set.to_string_lossy(), "action": "publish", "path": out.to_string_lossy() })).unwrap()
}

#[test]
fn a_drawings_empty_layouts_are_set_up_for_publishing_and_blank_sheets_warned_about() {
    hooks();
    let dir = folder("setup");
    bare_drawing(&dir.join("plan.dxf"), true);
    bare_drawing(&dir.join("blank.dxf"), false);
    let set = dir.join("Orchard Row.sheetset.json");
    let mut s = Session::new();
    s.execute("sheetset", &json!({ "set": set.to_string_lossy(), "action": "new" })).unwrap();
    s.execute("sheetset", &json!({ "set": set.to_string_lossy(), "action": "add", "drawing": dir.join("plan.dxf").to_string_lossy() })).unwrap();
    // The plan's layouts get a viewport on its extents: nothing to warn about, and the drawing's
    // own file is left as it was.
    let before = std::fs::read(dir.join("plan.dxf")).unwrap();
    let r = publish(&mut s, &set, &dir.join("a.pdf"));
    assert_eq!(r["pages"], 2);
    assert_eq!(r["warnings"], json!([]), "{r}");
    assert_eq!(std::fs::read(dir.join("plan.dxf")).unwrap(), before);
    // The plan shows on the page (a blank page is much smaller than one with the viewport's plan).
    let blank_pdf = {
        let mut t = Session::new();
        let set2 = dir.join("only-blank.sheetset.json");
        t.execute("sheetset", &json!({ "set": set2.to_string_lossy(), "action": "new" })).unwrap();
        t.execute(
            "sheetset",
            &json!({ "set": set2.to_string_lossy(), "action": "add", "drawing": dir.join("blank.dxf").to_string_lossy(), "layouts": ["Layout1"] }),
        )
        .unwrap();
        let log = t.log.len();
        let r = publish(&mut t, &set2, &dir.join("b.pdf"));
        // A sheet with nothing on it: warned in the result and on the command line.
        let w = r["warnings"].as_array().unwrap();
        assert_eq!(w.len(), 1, "{r}");
        assert!(w[0].as_str().unwrap().contains("nothing on it"), "{r}");
        assert!(t.log[log..].iter().any(|l| l.starts_with("Warning:") && l.contains("blank.dxf")), "{:?}", &t.log[log..]);
        std::fs::read(dir.join("b.pdf")).unwrap()
    };
    let one = dir.join("one-plan.sheetset.json");
    s.execute("sheetset", &json!({ "set": one.to_string_lossy(), "action": "new" })).unwrap();
    s.execute(
        "sheetset",
        &json!({ "set": one.to_string_lossy(), "action": "add", "drawing": dir.join("plan.dxf").to_string_lossy(), "layouts": ["Layout1"] }),
    )
    .unwrap();
    publish(&mut s, &one, &dir.join("one.pdf"));
    let plan_pdf = std::fs::read(dir.join("one.pdf")).unwrap();
    assert!(plan_pdf.len() > blank_pdf.len() + 400, "the plan is on its page: {} vs {}", plan_pdf.len(), blank_pdf.len());
    // Mixed: only the blank sheet is named.
    s.execute("sheetset", &json!({ "set": set.to_string_lossy(), "action": "add", "drawing": dir.join("blank.dxf").to_string_lossy(), "layouts": ["Layout2"], "number": "9", "title": "Spare" })).unwrap();
    let r = publish(&mut s, &set, &dir.join("c.pdf"));
    assert_eq!(r["pages"], 3);
    let w: Vec<&str> = r["warnings"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert_eq!(w.len(), 1, "{w:?}");
    assert!(w[0].starts_with("Sheet 9 Spare"), "{w:?}");
}

/// A drawing with layouts and a model space named view "Detail" (40 × 20 at 50,50).
fn drawing_with_view(path: &Path, layouts: &[&str]) {
    let mut d = Drawing::new_metric();
    d.layouts = layouts.iter().enumerate().map(|(i, n)| Layout::new(n, i as u32 + 1)).collect();
    d.views.push(cadaddy_engine::doc::NamedView {
        name: "Detail".into(),
        center: cadaddy_engine::doc::geom::Vec2::new(50.0, 50.0),
        height: 20.0,
        width: 40.0,
        ..Default::default()
    });
    std::fs::write(path, cadaddy_io::write(&d, &path.to_string_lossy()).unwrap()).unwrap();
}

fn text_of(d: &Drawing, hex: &str) -> String {
    let h = u64::from_str_radix(hex, 16).unwrap();
    match &d.entity(cadaddy_engine::doc::Handle(h)).unwrap().kind {
        EntityKind::MText(m) => m.contents.clone(),
        _ => String::new(),
    }
}

#[test]
fn views_callouts_and_fields_follow_renumbering() {
    hooks();
    let dir = folder("views");
    drawing_with_view(&dir.join("a.dxf"), &["Plan", "Details"]);
    let set = dir.join("v.sheetset.json").to_string_lossy().into_owned();
    let mut s = Session::new();
    let mut ss = |p: Value| {
        let mut p = p;
        p["set"] = json!(set);
        s.execute("sheetset", &p)
    };
    ss(json!({ "action": "new", "name": "Clinic", "description": "New clinic" })).unwrap();
    ss(json!({ "action": "add", "drawing": dir.join("a.dxf").to_string_lossy() })).unwrap();
    ss(json!({ "action": "renumber", "prefix": "A-", "start": 1 })).unwrap();
    // Custom properties of the set and a sheet.
    ss(json!({ "action": "properties", "values": { "Client": "City" } })).unwrap();
    let r = ss(json!({ "action": "properties", "sheet": "A-1", "values": { "Checked": "JD" }, "description": "Ground floor" })).unwrap();
    assert_eq!(r["properties"]["Client"], "City");
    assert_eq!(r["sheets"][0]["properties"]["Checked"], "JD");
    assert!(ss(json!({ "action": "properties", "values": { "a.b": "x" } })).is_err(), "no dots in names");

    // The detail view on the second sheet, a callout to it on the first.
    let r = ss(json!({ "action": "view", "sheet": "A-2", "view": "Detail", "at": [100, 100], "scale": 0.5 })).unwrap();
    assert_eq!(r["views"][0]["number"], "1");
    assert_eq!(r["views"][0]["sheet"], "A-2");
    let r = ss(json!({ "action": "callout", "sheet": "A-1", "view": "1/A-2", "at": [10, 10] })).unwrap();
    assert_eq!(r["callouts"][0]["target"], "A-2");
    let r = ss(json!({ "action": "fields" })).unwrap();
    let names: Vec<&str> = r["fields"].as_array().unwrap().iter().map(|f| f["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"SheetSet.Client") && names.contains(&"Sheet.Checked"), "{names:?}");
    ss(json!({ "action": "insertField", "sheet": "A-1", "field": "SheetSet.Client", "at": [5, 5] })).unwrap();
    assert!(ss(json!({ "action": "insertField", "sheet": "A-1", "field": "SheetSet.Nope", "at": [5, 5] })).is_err());

    let d = cadaddy_io::read(&std::fs::read(dir.join("a.dxf")).unwrap(), "a.dxf").unwrap();
    let vps = d.space(&Space::Paper("Details".into())).unwrap().iter().filter(|e| matches!(e.kind, EntityKind::Viewport(_))).count();
    assert!(vps >= 1, "the view's viewport is on the sheet");

    // Renumbered and synced: the label and the callout follow, and the fields read the set.
    ss(json!({ "action": "set", "sheet": "A-2", "number": "A-5" })).unwrap();
    let r = ss(json!({ "action": "sync" })).unwrap();
    let callout = r["callouts"][0]["handle"].as_str().unwrap().to_string();
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&set).unwrap()).unwrap();
    let label = v["views"][0]["label"].as_str().unwrap().to_string();
    let d = cadaddy_io::read(&std::fs::read(dir.join("a.dxf")).unwrap(), "a.dxf").unwrap();
    assert_eq!(text_of(&d, &callout), "1/A-5");
    assert_eq!(text_of(&d, &label), "1 Detail");

    s.execute("open", &json!({ "path": dir.join("a.dxf").to_string_lossy() })).unwrap();
    s.execute("layout", &json!({ "option": "set", "name": "A-1 Plan" })).unwrap();
    let f = |s: &mut Session, code: &str| {
        s.execute("field", &json!({ "field": "code", "code": code, "at": [0, 0] })).unwrap()["value"].as_str().unwrap().to_string()
    };
    assert_eq!(f(&mut s, "\\AcSm SheetSet.Name"), "Clinic");
    assert_eq!(f(&mut s, "\\AcSm SheetSet.Description"), "New clinic");
    assert_eq!(f(&mut s, "\\AcSm SheetSet.Client"), "City");
    assert_eq!(f(&mut s, "\\AcSm Sheet.Description"), "Ground floor");
    assert_eq!(f(&mut s, "\\AcSm Sheet.Checked"), "JD");
    assert_eq!(f(&mut s, "\\AcSm Sheet.Number"), "A-1");

    // Removing the sheet with the view drops the view and the callout pointing at it.
    let r = s.execute("sheetset", &json!({ "set": set, "action": "remove", "sheets": ["A-5"] })).unwrap();
    assert_eq!((r["views"].as_array().unwrap().len(), r["callouts"].as_array().unwrap().len()), (0, 0));
}

/// The page sizes (points) of a PDF's pages, from their MediaBox entries.
fn page_sizes(pdf: &[u8]) -> Vec<(i64, i64)> {
    let text = String::from_utf8_lossy(pdf);
    text.split("/MediaBox")
        .skip(1)
        .filter_map(|t| {
            let inner = t.split_once('[')?.1.split_once(']')?.0;
            let n: Vec<f64> = inner.split_whitespace().filter_map(|x| x.parse().ok()).collect();
            Some(((n.get(2)? - n.first()?).round() as i64, (n.get(3)? - n.get(1)?).round() as i64))
        })
        .collect()
}

#[test]
fn a_page_setup_override_changes_the_published_paper() {
    hooks();
    let dir = folder("pages");
    drawing(&dir.join("a.dxf"), &["L1", "L2"]);
    let set = dir.join("p.sheetset.json").to_string_lossy().into_owned();
    let mut s = Session::new();
    s.execute("sheetset", &json!({ "set": set, "action": "new" })).unwrap();
    s.execute("sheetset", &json!({ "set": set, "action": "add", "drawing": dir.join("a.dxf").to_string_lossy() })).unwrap();
    let out = dir.join("own.pdf");
    s.execute("sheetset", &json!({ "set": set, "action": "publish", "path": out.to_string_lossy() })).unwrap();
    let own = page_sizes(&std::fs::read(&out).unwrap());
    assert_eq!(own.len(), 2, "{own:?}");

    // An A1 landscape setup for every sheet.
    let r = s
        .execute(
            "sheetset",
            &json!({ "set": set, "action": "pagesetup", "name": "A1", "page": { "paper": "ISO A1", "widthMm": 841.0, "heightMm": 594.0, "landscape": false }, "use": "A1" }),
        )
        .unwrap();
    assert_eq!(r["pageSetup"], "A1");
    let out2 = dir.join("a1.pdf");
    s.execute("sheetset", &json!({ "set": set, "action": "publish", "path": out2.to_string_lossy() })).unwrap();
    let a1 = page_sizes(&std::fs::read(&out2).unwrap());
    assert_eq!(a1.len(), 2);
    assert_ne!(a1, own);
    // 841 mm ≈ 2384 pt on one side.
    assert!(a1.iter().all(|(w, h)| (*w - 2384).abs() <= 2 || (*h - 2384).abs() <= 2), "{a1:?}");
    // Unknown setups and setups without paper are refused; removing the one used clears it.
    assert!(s.execute("sheetset", &json!({ "set": set, "action": "pagesetup", "use": "Nope" })).is_err());
    assert!(s.execute("sheetset", &json!({ "set": set, "action": "pagesetup", "name": "Z", "page": { "widthMm": 0 } })).is_err());
    let r = s.execute("sheetset", &json!({ "set": set, "action": "pagesetup", "remove": "A1" })).unwrap();
    assert_eq!(r["pageSetup"], "");
}
