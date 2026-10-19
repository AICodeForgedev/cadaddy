//! Hyperlinks (HYPERLINK, -HYPERLINK, HYPERLINKBASE, following a link): kept as `PE_URL` extended
//! data, so DXF, DWG and PDF carry them.

use cadaddy_engine::Session;
use cadaddy_engine::doc::Handle;
use serde_json::{Value, json};

fn drawn() -> (Session, Handle, Handle) {
    let mut s = Session::new();
    s.execute("line", &json!({"points": [[0, 0], [10, 0]]})).unwrap();
    s.execute("circle", &json!({"center": [5, 5], "radius": 2})).unwrap();
    let hs: Vec<Handle> = s.doc().unwrap().model.handles();
    (s, hs[0], hs[1])
}

fn query(s: &mut Session, h: Handle) -> Value {
    s.execute("hyperlink", &json!({"handles": [h.hex()]})).unwrap()["links"].clone()
}

#[test]
fn insert_edit_and_remove_a_hyperlink() {
    let (mut s, line, circle) = drawn();
    let r = s
        .execute("hyperlink", &json!({"handles": [line.hex(), circle.hex()], "url": "https://example.com/spec", "description": "Specification"}))
        .unwrap();
    assert_eq!(r["changed"], json!(2));
    assert_eq!(query(&mut s, line), json!([{"handle": line.hex(), "url": "https://example.com/spec", "target": "", "description": "Specification"}]));
    // Kept as AutoCAD keeps it.
    let x: Vec<(i32, String)> = s.doc().unwrap().entity(line).unwrap().common.xdata.iter().map(|t| (t.code, t.value.clone())).collect();
    assert_eq!(
        x,
        [
            (1001, "PE_URL"),
            (1000, "https://example.com/spec"),
            (1002, "{"),
            (1000, "Specification"),
            (1002, "{"),
            (1071, "0"),
            (1002, "}"),
            (1002, "}")
        ]
        .map(|(c, v)| (c, v.to_string()))
    );
    // Edited: one undo step brings the old link back.
    s.execute("hyperlink", &json!({"handles": [line.hex()], "url": "plans/site.dwg", "target": "Layout1"})).unwrap();
    assert_eq!(query(&mut s, line)[0]["target"], json!("Layout1"));
    s.undo().unwrap();
    assert_eq!(query(&mut s, line)[0]["url"], json!("https://example.com/spec"));
    // Removed from the selection.
    s.set_selection(vec![circle]);
    assert_eq!(s.execute("hyperlink", &json!({"remove": true})).unwrap()["changed"], json!(1));
    assert_eq!(query(&mut s, circle), json!([]));
    // Bad calls are refused, not panics.
    assert!(s.execute("hyperlink", &json!({"handles": [line.hex()], "url": ""})).is_err(), "neither url nor target");
    assert!(s.execute("hyperlink", &json!({"handles": [line.hex()], "url": 5})).is_err());
    assert!(s.execute("hyperlink", &json!({"handles": [], "url": "x"})).is_err());
    assert!(s.execute("hyperlink", &json!({"handles": ["zz"], "url": "x"})).is_err());
    let r = s.execute("hyperlink", &json!({"handles": [line.hex()], "url": "u".repeat(100_000)})).unwrap();
    assert_eq!(r["changed"], json!(1));
}

#[test]
fn dash_hyperlink_at_the_command_line() {
    let (mut s, line, circle) = drawn();
    s.set_selection(vec![line]);
    s.script("-hyperlink\ni\nhttps://example.com/a\n\nDoor 7\n").unwrap();
    assert_eq!(query(&mut s, line)[0]["description"], json!("Door 7"));
    assert!(s.log.iter().any(|l| l.contains("1 hyperlink attached")), "{:?}", s.log);
    // A link into this drawing: no URL, a named location.
    s.set_selection(vec![circle]);
    s.script("-hyperlink\n\n\nLayout1\n\n").unwrap();
    assert_eq!(query(&mut s, circle)[0]["target"], json!("Layout1"));
    // Remove.
    s.set_selection(vec![line, circle]);
    s.script("-hyperlink\nr\n").unwrap();
    assert!(s.log.iter().any(|l| l.contains("2 hyperlinks deleted")), "{:?}", s.log);
    assert_eq!(query(&mut s, line), json!([]));
}

#[test]
fn following_a_link_resolves_it_or_goes_to_the_named_location() {
    let (mut s, line, circle) = drawn();
    s.execute("hyperlink", &json!({"handles": [line.hex()], "url": "docs/spec.pdf"})).unwrap();
    s.execute("hyperlink", &json!({"handles": [circle.hex()], "target": "Layout1", "description": "Sheet"})).unwrap();
    assert_eq!(s.execute("getvar", &json!({"name": "HYPERLINKBASE"})).unwrap()["value"], json!(""));
    s.execute("setvar", &json!({"name": "HYPERLINKBASE", "value": "https://example.com/project"})).unwrap();
    let r = s.execute("hyperlink.open", &json!({"handle": line.hex()})).unwrap();
    assert_eq!(r["url"], json!("https://example.com/project/docs/spec.pdf"));
    // A base that looks like a number stays text.
    s.execute("setvar", &json!({"name": "HYPERLINKBASE", "value": "2024"})).unwrap();
    assert_eq!(s.execute("getvar", &json!({"name": "HYPERLINKBASE"})).unwrap()["value"], json!("2024"));
    // A named location of this drawing: the engine goes there.
    let r = s.execute("hyperlink.open", &json!({"handle": circle.hex()})).unwrap();
    assert_eq!((r["target"].clone(), r["label"].clone()), (json!("Layout1"), json!("Sheet")));
    assert_eq!(s.execute("getvar", &json!({"name": "CTAB"})).unwrap()["value"], json!("Layout1"));
    // No link, or a location the drawing doesn't have: an error.
    s.execute("layout", &json!({"option": "set", "name": "Model"})).unwrap();
    s.execute("hyperlink", &json!({"handles": [circle.hex()], "target": "Nowhere"})).unwrap();
    assert!(s.execute("hyperlink.open", &json!({"handle": circle.hex()})).is_err());
    s.execute("hyperlink", &json!({"handles": [circle.hex()], "remove": true})).unwrap();
    assert!(s.execute("hyperlink.open", &json!({"handle": circle.hex()})).is_err());
    assert!(s.execute("hyperlink.open", &json!({})).is_err());
}

#[test]
fn hyperlinks_survive_dxf_and_dwg_and_link_in_pdf() {
    let (mut s, line, _) = drawn();
    s.execute("hyperlink", &json!({"handles": [line.hex()], "url": "https://example.com/spec", "target": "part2", "description": "Spec"})).unwrap();
    s.execute("setvar", &json!({"name": "HYPERLINKBASE", "value": "https://example.com/base/"})).unwrap();
    let d = s.doc().unwrap().clone();
    for name in ["x.dxf", "x.dwg"] {
        let bytes = cadaddy_io::write(&d, name).unwrap();
        let back = cadaddy_io::read(&bytes, name).unwrap();
        let link = back.model.iter().find_map(|e| cadaddy_engine::doc::hyperlink::read(&e.common.xdata));
        assert_eq!(
            link.map(|l| (l.url, l.target, l.description)),
            Some(("https://example.com/spec".into(), "part2".into(), "Spec".into())),
            "{name}"
        );
        assert_eq!(back.header.str("HYPERLINKBASE", "?"), "https://example.com/base/", "{name}");
        if name == "x.dxf" {
            // Registered once in the APPID table (AutoCAD refuses extended data of unregistered
            // applications).
            let text = String::from_utf8_lossy(&bytes);
            let lines: Vec<&str> = text.lines().map(str::trim).collect();
            let appids = lines.windows(3).filter(|w| w[0] == "AcDbRegAppTableRecord" && w[1] == "2" && w[2] == "PE_URL").count();
            assert_eq!(appids, 1, "one APPID record for PE_URL");
        }
    }
    let pdf = cadaddy_io::pdf(&d, &cadaddy_engine::doc::Space::Model, &cadaddy_io::PdfOptions::default()).unwrap();
    let text = String::from_utf8_lossy(&pdf);
    assert!(text.contains("https://example.com/spec#part2"), "the link annotation");
}
