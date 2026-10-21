//! Fields (FIELD, UPDATEFIELD, FIELDEVAL, FIELDDISPLAY): codes in text show their values —
//! dates, the file name, sheet names from layouts, object properties, block attributes — worked
//! out on open/save/plot/regen as FIELDEVAL says, kept through copies and DXF.

use cadaddy_engine::Session;
use cadaddy_engine::doc::{EntityKind, Handle, Space};
use cadaddy_engine::fields::{self, Event, LocalTime};
use cadaddy_engine::render;
use serde_json::{Value, json};

/// 2026-10-18 14:30:05, a Sunday.
fn clock() -> Option<LocalTime> {
    Some(LocalTime { year: 2026, month: 10, day: 18, hour: 14, minute: 30, second: 5, weekday: 0 })
}

fn session() -> Session {
    fields::set_clock(Some(clock));
    let mut s = Session::new();
    s.viewport_px = (1200.0, 800.0);
    s
}

fn hd(v: &Value) -> Handle {
    Handle::parse_hex(v["handle"].as_str().unwrap()).unwrap()
}

fn values(s: &Session, h: Handle) -> Vec<String> {
    s.doc().unwrap().field_values.get(&h).cloned().unwrap_or_default()
}

fn field(s: &mut Session, p: Value) -> (Handle, String) {
    let r = s.execute("field", &p).unwrap();
    (hd(&r), r["value"].as_str().unwrap().to_string())
}

#[test]
fn dates_and_their_formats() {
    let mut s = session();
    assert_eq!(field(&mut s, json!({ "field": "date", "at": [0, 0] })).1, "10/18/2026");
    assert_eq!(field(&mut s, json!({ "field": "date", "format": "dddd, MMMM d, yyyy", "at": [0, 5] })).1, "Sunday, October 18, 2026");
    assert_eq!(field(&mut s, json!({ "field": "date", "format": "yyyy-MM-dd HH:mm:ss", "at": [0, 10] })).1, "2026-10-18 14:30:05");
    assert_eq!(field(&mut s, json!({ "field": "date", "format": "h:mm tt", "at": [0, 15] })).1, "2:30 PM");
    // Not saved or plotted yet.
    assert_eq!(field(&mut s, json!({ "field": "savedate", "at": [0, 20] })).1, "####");
    assert_eq!(field(&mut s, json!({ "field": "plotdate", "at": [0, 25] })).1, "####");
}

#[test]
fn plot_and_save_dates_follow_fieldeval() {
    let mut s = session();
    let (plot, _) = field(&mut s, json!({ "field": "plotdate", "at": [0, 0] }));
    let (save, _) = field(&mut s, json!({ "field": "savedate", "at": [0, 5] }));
    // Plotting works PlotDate out; other events keep it.
    assert_eq!(fields::on_event(&mut s, Event::Plot), 1);
    assert_eq!(values(&s, plot), ["10/18/2026"]);
    s.execute("regen", &json!({})).unwrap();
    assert_eq!(values(&s, plot), ["10/18/2026"]);
    // Saving sets the save date first.
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
    });
    let dir = std::env::temp_dir().join(format!("cadaddy-fields-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Plan.dxf");
    s.execute("saveas", &json!({ "path": path.to_string_lossy() })).unwrap();
    assert_eq!(values(&s, save), ["10/18/2026"]);
    let tdupdate = s.doc().unwrap().header.f64("TDUPDATE", 0.0);
    assert!((tdupdate - clock().unwrap().julian()).abs() < 1e-6, "{tdupdate}");
    // Opened again: the values come back (and opening works them out without changing the drawing).
    s.execute("close", &json!({})).unwrap();
    s.execute("open", &json!({ "path": path.to_string_lossy() })).unwrap();
    assert_eq!(s.state().unwrap().path.as_deref(), Some(path.to_string_lossy().as_ref()));
    assert_eq!(values(&s, save), ["10/18/2026"]);
    assert!(!s.state().unwrap().is_dirty());
    // FIELDEVAL 0: no event works fields out; UPDATEFIELD still does.
    s.execute("setvar", &json!({ "name": "FIELDEVAL", "value": 0 })).unwrap();
    let (sheet, _) = field(&mut s, json!({ "field": "sheetname", "at": [0, 10] }));
    s.execute("layout", &json!({ "option": "set", "name": "Model" })).ok();
    std::sync::Arc::make_mut(&mut s.state_mut().unwrap().doc).field_values.insert(sheet, vec!["old".into()]);
    s.execute("regen", &json!({})).unwrap();
    assert_eq!(values(&s, sheet), ["old"]);
    let r = s.execute("updatefield", &json!({ "handles": [sheet.hex()] })).unwrap();
    assert_eq!(r["updated"], 1);
    assert_eq!(values(&s, sheet), ["Model"]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_file_name_in_its_formats() {
    let mut s = session();
    s.state_mut().unwrap().path = Some("/jobs/site/Plan A.dwg".into());
    let f = |s: &mut Session, fmt: &str| field(s, json!({ "field": "filename", "format": fmt, "at": [0, 0] })).1;
    assert_eq!(f(&mut s, "%fn2"), "/jobs/site/Plan A.dwg");
    assert_eq!(f(&mut s, "%fn6"), "Plan A.dwg");
    assert_eq!(f(&mut s, "%fn4"), "Plan A");
    assert_eq!(f(&mut s, "%fn0"), "/jobs/site/Plan A");
    assert_eq!(f(&mut s, "%fn1"), "/jobs/site");
    assert_eq!(f(&mut s, "%fn6%tc1"), "PLAN A.DWG");
    // The default is the file name with its extension.
    assert_eq!(field(&mut s, json!({ "field": "filename", "at": [0, 0] })).1, "Plan A.dwg");
    // A path from Windows splits the same on every platform and keeps its separators.
    s.state_mut().unwrap().path = Some(r"D:\jobs\site.v2\Plan B.dxf".into());
    assert_eq!(f(&mut s, "%fn2"), r"D:\jobs\site.v2\Plan B.dxf");
    assert_eq!(f(&mut s, "%fn0"), r"D:\jobs\site.v2\Plan B");
    assert_eq!(f(&mut s, "%fn1"), r"D:\jobs\site.v2");
    assert_eq!(f(&mut s, "%fn4"), "Plan B");
    // No extension, a dot in the folder, a name that starts with a dot, the root folder.
    s.state_mut().unwrap().path = Some("/jobs/site.v2/Plan".into());
    assert_eq!(f(&mut s, "%fn0"), "/jobs/site.v2/Plan");
    s.state_mut().unwrap().path = Some("/jobs/.plan".into());
    assert_eq!(f(&mut s, "%fn4"), ".plan");
    s.state_mut().unwrap().path = Some("/Plan.dwg".into());
    assert_eq!(f(&mut s, "%fn1"), "/");
    assert_eq!(f(&mut s, "%fn0"), "/Plan");
}

#[test]
fn sheet_fields_come_from_layout_names() {
    let mut s = session();
    let layout = s.doc().unwrap().layouts[0].name.clone();
    s.execute("layout.rename", &json!({ "from": layout, "to": "A-101 Floor Plan" })).unwrap();
    s.execute("layout", &json!({ "option": "set", "name": "A-101 Floor Plan" })).unwrap();
    let (n, number) = field(&mut s, json!({ "field": "sheetnumber", "at": [1, 1] }));
    assert_eq!(number, "A-101");
    assert_eq!(field(&mut s, json!({ "field": "sheettitle", "at": [1, 2] })).1, "Floor Plan");
    assert_eq!(field(&mut s, json!({ "field": "sheetname", "at": [1, 3] })).1, "A-101 Floor Plan");
    assert_eq!(field(&mut s, json!({ "field": "code", "code": "\\AcSm Sheet.Title \\f \"%tc1\"", "at": [1, 4] })).1, "FLOOR PLAN");
    // Renamed: a regen follows.
    s.execute("layout.rename", &json!({ "from": "A-101 Floor Plan", "to": "E-2 Lighting" })).unwrap();
    s.execute("regen", &json!({})).unwrap();
    assert_eq!(values(&s, n), ["E-2"]);
    // In model space the sheet is "Model".
    s.execute("layout", &json!({ "option": "set", "name": "Model" })).unwrap();
    assert_eq!(field(&mut s, json!({ "field": "sheetname", "at": [0, 0] })).1, "Model");
}

#[test]
fn object_properties_update_on_regen() {
    let mut s = session();
    let pl = hd(&s.execute("pline", &json!({ "vertices": [[0, 0], [4, 0], [4, 3], [0, 3]], "closed": true })).unwrap());
    let (area, v) = field(&mut s, json!({ "field": "object", "object": pl.hex(), "property": "Area", "format": "%lu2%pr2", "at": [0, -5] }));
    assert_eq!(v, "12.00");
    let (len, v) =
        field(&mut s, json!({ "field": "object", "object": pl.hex(), "property": "Perimeter", "format": "%lu2%pr1%ps[P=, m]", "at": [0, -7] }));
    assert_eq!(v, "P=14.0 m");
    let c = hd(&s.execute("circle", &json!({ "center": [10, 0], "radius": 2 })).unwrap());
    assert_eq!(field(&mut s, json!({ "field": "object", "object": c.hex(), "property": "Diameter", "format": "%lu2%pr0", "at": [0, -9] })).1, "4");
    assert_eq!(field(&mut s, json!({ "field": "object", "object": c.hex(), "property": "Layer", "at": [0, -11] })).1, "0");
    // A factor: square inches as square feet.
    assert_eq!(
        field(&mut s, json!({ "field": "object", "object": pl.hex(), "property": "Area", "format": "%lu2%pr3%ct8[0.5]", "at": [0, -13] })).1,
        "6.000"
    );
    // The object changes: the field shows the old value until the next regen.
    s.execute("scale", &json!({ "handles": [pl.hex()], "base": [0, 0], "factor": 2 })).unwrap();
    assert_eq!(values(&s, area), ["12.00"]);
    s.execute("regen", &json!({})).unwrap();
    assert_eq!(values(&s, area), ["48.00"]);
    assert_eq!(values(&s, len), ["P=28.0 m"]);
    // Gone: the field shows ####.
    s.execute("erase", &json!({ "handles": [pl.hex()] })).unwrap();
    s.execute("regen", &json!({})).unwrap();
    assert_eq!(values(&s, area), ["####"]);
    for bad in [
        json!({ "field": "object", "property": "Area", "at": [0, 0] }),
        json!({ "field": "object", "object": c.hex(), "property": "Flavour", "at": [0, 0] }),
        json!({ "field": "object", "object": "FFFFFF", "property": "Area", "at": [0, 0] }),
        json!({ "field": "weather", "at": [0, 0] }),
        json!({ "field": "date", "format": "a\"b", "at": [0, 0] }),
        json!({ "field": "code", "code": "", "at": [0, 0] }),
        json!({ "field": "date" }),
    ] {
        assert!(s.execute("field", &bad).is_err(), "{bad}");
    }
}

#[test]
fn block_attributes_and_attribute_fields() {
    let mut s = session();
    let c = s.execute("circle", &json!({ "center": [0, 0], "radius": 1 })).unwrap();
    let a = s.execute("attdef", &json!({ "tag": "ROOM", "default": "101", "at": [0, 0] })).unwrap();
    let d = s.execute("attdef", &json!({ "tag": "DATE", "default": "%<\\AcVar Date \\f \"yyyy\">%", "at": [0, -1] })).unwrap();
    s.execute("block", &json!({ "name": "TAG", "base": [0, 0], "handles": [c["handle"], a["handle"], d["handle"]], "keep": "delete" })).unwrap();
    let ins = hd(&s.execute("insert", &json!({ "name": "TAG", "at": [5, 5], "attribs": { "ROOM": "204" } })).unwrap());
    // An attribute whose default is a field shows the field's value.
    assert_eq!(values(&s, ins), ["2026"]);
    // Another text shows the block reference's attribute.
    let (h, v) = field(&mut s, json!({ "field": "object", "object": ins.hex(), "property": "Attribute(room)", "at": [0, 0] }));
    assert_eq!(v, "204");
    s.execute("attedit", &json!({ "handle": ins.hex(), "tag": "ROOM", "value": "305" })).ok();
    s.execute("updatefield", &json!({ "handles": [h.hex()] })).unwrap();
    let room = s.doc().unwrap().entity(ins).map(|e| match &e.kind {
        EntityKind::Insert(i) => i.attribs.iter().find(|a| a.tag == "ROOM").map(|a| a.text.value.clone()),
        _ => None,
    });
    assert_eq!(values(&s, h), [room.flatten().unwrap()]);
}

#[test]
fn copies_and_edits_keep_values_and_undo_takes_them_back() {
    let mut s = session();
    let (h, _) = field(&mut s, json!({ "field": "date", "at": [0, 0] }));
    let r = s.execute("copy", &json!({ "handles": [h.hex()], "from": [0, 0], "to": [0, 5] })).unwrap();
    let copy = r["handles"].as_array().and_then(|a| a.first()).and_then(Value::as_str).and_then(Handle::parse_hex).unwrap();
    assert_eq!(values(&s, copy), ["10/18/2026"], "a copy's fields are worked out");
    // A field added to an existing text.
    let t = hd(&s.execute("text", &json!({ "at": [0, 10], "text": "Printed: ", "height": 1 })).unwrap());
    field(&mut s, json!({ "field": "date", "format": "MMM d", "into": t.hex() }));
    assert_eq!(values(&s, t), ["Oct 18"]);
    s.undo().unwrap();
    assert!(values(&s, t).is_empty());
    // Text typed with a field code in it gets its value too.
    let typed = hd(&s.execute("mtext", &json!({ "at": [0, 20], "text": "On %<\\AcVar Date \\f \"d.M.yy\">%" })).unwrap());
    assert_eq!(values(&s, typed), ["18.10.26"]);
}

/// The text a display list draws for `h`, as the points of its strokes (to compare drawings).
fn drawn(s: &Session, h: Handle, shade: bool) -> (usize, Vec<render::DPrim>) {
    let d = s.doc().unwrap();
    let l = render::build(d, &Space::Model, &render::Options { field_shading: shade, ..render::Options::default() });
    let prims: Vec<render::DPrim> = l.prims.iter().filter(|p| p.handle == h).cloned().collect();
    (prims.len(), prims)
}

#[test]
fn fields_draw_their_values_and_fielddisplay_shades_them() {
    let mut s = session();
    let (h, _) = field(&mut s, json!({ "field": "date", "at": [0, 0], "height": 1 }));
    let plain = hd(&s.execute("mtext", &json!({ "at": [0, 5], "text": "10/18/2026", "height": 1 })).unwrap());
    // Drawn exactly as the same text typed out.
    let (n_field, _) = drawn(&s, h, false);
    let (n_plain, _) = drawn(&s, plain, false);
    assert_eq!(n_field, n_plain);
    // Shaded on screen: one more piece (the grey box), never for plain text.
    assert!(drawn(&s, h, true).0 > n_field);
    assert_eq!(drawn(&s, plain, true).0, n_plain);
    // Single-line text too.
    let t = hd(&s.execute("text", &json!({ "at": [0, 10], "text": "x", "height": 1 })).unwrap());
    field(&mut s, json!({ "field": "date", "into": t.hex() }));
    assert!(drawn(&s, t, true).0 > drawn(&s, t, false).0);
    assert_eq!(s.execute("getvar", &json!({ "name": "FIELDDISPLAY" })).unwrap()["value"], 1);
    // Without a value: ####.
    s.doc_mut().unwrap().field_values.clear();
    let (n_none, _) = drawn(&s, h, false);
    let hashes = hd(&s.execute("mtext", &json!({ "at": [0, 15], "text": "####", "height": 1 })).unwrap());
    assert_eq!(n_none, drawn(&s, hashes, false).0);
}

#[test]
fn fields_round_trip_through_dxf() {
    let mut s = session();
    let pl = hd(&s.execute("pline", &json!({ "vertices": [[0, 0], [2, 0], [2, 2]], "closed": true })).unwrap());
    let (m, _) = field(&mut s, json!({ "field": "date", "format": "yyyy", "at": [0, 0] }));
    field(&mut s, json!({ "field": "object", "object": pl.hex(), "property": "Area", "format": "%lu2%pr1", "into": m.hex() }));
    let t = hd(&s.execute("text", &json!({ "at": [0, 5], "text": "Sheet ", "height": 1 })).unwrap());
    field(&mut s, json!({ "field": "sheetname", "into": t.hex() }));
    let c = s.execute("circle", &json!({ "center": [0, 0], "radius": 1 })).unwrap();
    let a = s.execute("attdef", &json!({ "tag": "D", "default": "%<\\AcVar Date \\f \"M/yyyy\">%", "at": [0, 0] })).unwrap();
    s.execute("block", &json!({ "name": "B", "base": [0, 0], "handles": [c["handle"], a["handle"]], "keep": "delete" })).unwrap();
    let ins = hd(&s.execute("insert", &json!({ "name": "B", "at": [9, 9] })).unwrap());
    let d = s.doc().unwrap();
    let text = cadaddy_io::write_dxf(d);
    // Other programs see the values; the codes are in FIELD objects.
    assert!(text.contains("ACAD_FIELD") && text.contains("\\AcVar Date \\f \"yyyy\"") && text.contains("%<\\_FldIdx 0>%"));
    assert!(text.contains("\\AcObjProp Object(%<\\_ObjIdx 0>%).Area"));
    assert!(text.contains("\n2026") && text.contains("Sheet Model"));
    let back = cadaddy_io::read_dxf(text.as_bytes()).unwrap();
    for h in [m, t, ins] {
        let (a, b) = (d.entity(h).unwrap(), back.entity(h).unwrap());
        assert_eq!(cadaddy_engine::doc::field::texts(&a.kind), cadaddy_engine::doc::field::texts(&b.kind), "{}", h.hex());
        assert_eq!(d.field_values.get(&h), back.field_values.get(&h), "{}", h.hex());
    }
    // And again: nothing doubles.
    let again = cadaddy_io::write_dxf(&back);
    assert_eq!(again.matches("ACAD_FIELD").count(), text.matches("ACAD_FIELD").count());
    assert_eq!(again.lines().filter(|l| l.trim() == "FIELD").count(), text.lines().filter(|l| l.trim() == "FIELD").count());
}

#[test]
fn typed_field_and_updatefield() {
    let mut s = session();
    for l in ["field", "Date", "MMMM yyyy"] {
        s.cmdline(l).unwrap();
    }
    s.input(cadaddy_engine::Input::Point(cadaddy_engine::geom::Vec2::new(3.0, 4.0))).unwrap();
    assert!(s.running.is_none(), "{}", s.prompt_text());
    let last = s.doc().unwrap().model.last().map(|e| e.handle).unwrap();
    assert_eq!(values(&s, last), ["October 2026"]);
    // Object, picked; a bad property re-prompts at the format.
    let c = hd(&s.execute("circle", &json!({ "center": [0, 0], "radius": 3 })).unwrap());
    s.cmdline("field").unwrap();
    s.cmdline("o").unwrap();
    s.input(cadaddy_engine::Input::Pick(vec![c])).unwrap();
    s.cmdline("Radius").unwrap();
    s.cmdline("%lu2%pr1").unwrap();
    s.input(cadaddy_engine::Input::Point(cadaddy_engine::geom::Vec2::new(0.0, 9.0))).unwrap();
    let last = s.doc().unwrap().model.last().map(|e| e.handle).unwrap();
    assert_eq!(values(&s, last), ["3.0"]);
    // UPDATEFIELD typed: select, Enter.
    s.cmdline("updatefield").unwrap();
    s.input(cadaddy_engine::Input::Pick(vec![last])).unwrap();
    s.input(cadaddy_engine::Input::Enter).unwrap();
    assert!(s.log.iter().any(|l| l.contains("1 field object(s) updated")), "{:?}", s.log);
}

#[test]
fn hostile_field_codes_never_panic() {
    let mut s = session();
    for text in [
        "%<\\AcVar>%",
        "%<\\>%",
        "%<\\AcObjProp Object(%<\\_ObjId 99999999999999999999>%).Area>%",
        "%<\\AcObjProp Object(%<\\_ObjId>%).>%",
        "%<\\AcVar Date \\f \"",
        "%<\\AcVar Date \\f \"dddddddddddddddddddddddddddddddddddd''''''''yyyyyyyyyyyyyyyy\">%",
        "%<\\AcVar Filename \\f \"%fn9%tc9\">%",
        "%<\\AcVar %<\\AcVar %<\\AcVar Date>%>%>%",
        &"%<\\AcVar Date>%".repeat(1500),
        &format!("{}x{}", "%<".repeat(3000), ">%".repeat(3000)),
    ] {
        let r = s.execute("mtext", &json!({ "at": [0, 0], "text": text }));
        assert!(r.is_ok(), "{text}");
    }
    s.execute("regen", &json!({})).unwrap();
    s.execute("updatefield", &json!({})).unwrap();
    let d = s.doc().unwrap();
    let _ = render::build(d, &Space::Model, &render::Options { field_shading: true, ..render::Options::default() });
    let text = cadaddy_io::write_dxf(d);
    assert!(cadaddy_io::read_dxf(text.as_bytes()).is_ok());
}

/// Fields stay fields through a DWG file (tester 0.7.7f: after saving as .dwg and opening it
/// again, UPDATEFIELD found 0 and the texts were static).
#[test]
fn fields_survive_a_dwg_round_trip() {
    let mut s = session();
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
    });
    let (date, shown) = field(&mut s, json!({ "field": "date", "format": "yyyy-MM-dd", "at": [0, 0] }));
    let (file, _) = field(&mut s, json!({ "field": "filename", "at": [0, 5] }));
    let dir = std::env::temp_dir().join(format!("cadaddy-fields-dwg-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Sheet.dwg");
    s.execute("saveas", &json!({ "path": path.to_string_lossy() })).unwrap();
    s.execute("close", &json!({})).unwrap();
    s.execute("open", &json!({ "path": path.to_string_lossy() })).unwrap();
    // The values come back, and they are still fields: opening worked the file name out again.
    assert_eq!(values(&s, date), [shown]);
    assert_eq!(values(&s, file), ["Sheet.dwg"]);
    let r = s.execute("updatefield", &json!({})).unwrap();
    assert_eq!(r["updated"], 2, "{r}");
    let _ = std::fs::remove_dir_all(&dir);
}
