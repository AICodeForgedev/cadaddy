//! Express Tools IV: BCOUNT, GATTE, GETSEL, SSX, XDLIST, XLIST, SHOWURLS, REDIR, REDIRMODE,
//! MPEDIT, LSP, LSPSURF, REVERT, TFRAMES, BSCALE, BROTATE, VPSYNC and DIMREASSOC — JSON forms,
//! typed forms, and bad input refused.

use cadaddy_engine::Session;
use cadaddy_engine::doc::{EntityKind, Handle, RawTag};
use serde_json::{Value, json};

fn h(v: &Value) -> Handle {
    Handle::parse_hex(v["handle"].as_str().unwrap()).unwrap()
}

fn hexes(hs: &[Handle]) -> Vec<String> {
    hs.iter().map(|h| h.hex()).collect()
}

fn last(s: &Session) -> Handle {
    s.doc().unwrap().model.iter().last().unwrap().handle
}

/// Block INNER (a circle), block OUTER (a line, two INNERs and a TAG attribute), and in model
/// space two OUTERs and one INNER: the references' handles.
fn blocks() -> (Session, Vec<Handle>) {
    let mut s = Session::new();
    s.execute("circle", &json!({"center": [0, 0], "radius": 1})).unwrap();
    let c = last(&s);
    s.execute("block", &json!({"name": "INNER", "base": [0, 0], "handles": [c.hex()], "keep": "delete"})).unwrap();
    s.execute("line", &json!({"points": [[0, 0], [10, 0]]})).unwrap();
    let l = last(&s);
    let i1 = h(&s.execute("insert", &json!({"name": "INNER", "at": [2, 2]})).unwrap());
    let i2 = h(&s.execute("insert", &json!({"name": "INNER", "at": [8, 2]})).unwrap());
    s.execute("attdef", &json!({"tag": "TAG", "default": "T0", "at": [0, -2], "height": 1})).unwrap();
    let a = last(&s);
    s.execute("block", &json!({"name": "OUTER", "base": [0, 0], "handles": hexes(&[l, i1, i2, a]), "keep": "delete"})).unwrap();
    let o1 = h(&s.execute("insert", &json!({"name": "OUTER", "at": [0, 0], "attribs": {"TAG": "A-1"}})).unwrap());
    let o2 = h(&s.execute("insert", &json!({"name": "OUTER", "at": [50, 0], "attribs": {"TAG": "B-2"}})).unwrap());
    let i3 = h(&s.execute("insert", &json!({"name": "INNER", "at": [100, 0]})).unwrap());
    s.set_selection(Vec::new());
    (s, vec![o1, o2, i3])
}

fn insert(s: &Session, h: Handle) -> cadaddy_engine::doc::Insert {
    match &s.doc().unwrap().entity(h).unwrap().kind {
        EntityKind::Insert(i) => i.clone(),
        k => panic!("not an insert: {k:?}"),
    }
}

fn attr(s: &Session, h: Handle, tag: &str) -> String {
    insert(s, h).attribs.iter().find(|a| a.tag == tag).unwrap().text.value.clone()
}

#[test]
fn bcount_counts_nested_references_once_per_definition() {
    let (mut s, refs) = blocks();
    let r = s.execute("bcount", &json!({})).unwrap();
    // 2 OUTER + 1 INNER at the top, 2 INNER inside each OUTER.
    assert_eq!(r["counts"], json!({"INNER": 5, "OUTER": 2}));
    assert_eq!(r["total"], 7);
    let r = s.execute("bcount", &json!({"nested": false})).unwrap();
    assert_eq!(r["counts"], json!({"INNER": 1, "OUTER": 2}));
    let r = s.execute("bcount", &json!({"handles": [refs[0].hex()]})).unwrap();
    assert_eq!(r["counts"], json!({"INNER": 2, "OUTER": 1}));
    assert!(r["message"].as_str().unwrap().contains("OUTER"));
    // Typed: Enter at "Select objects" counts everything.
    s.cmdline("bcount").unwrap();
    s.cmdline("").unwrap();
    assert!(s.running.is_none());
    assert!(s.log.join("\n").contains("INNER"), "{}", s.log.join("\n"));
    assert!(s.execute("bcount", &json!({"handles": ["zz"]})).is_err());
}

#[test]
fn gatte_sets_an_attribute_in_every_reference() {
    let (mut s, refs) = blocks();
    let r = s.execute("gatte", &json!({"block": "outer", "tag": "tag", "value": "NEW"})).unwrap();
    assert_eq!(r["changed"], 2);
    assert_eq!(attr(&s, refs[0], "TAG"), "NEW");
    assert_eq!(attr(&s, refs[1], "TAG"), "NEW");
    // Only some references; one undo step.
    s.execute("gatte", &json!({"block": "OUTER", "tag": "TAG", "value": "ONE", "handles": [refs[1].hex()]})).unwrap();
    assert_eq!(attr(&s, refs[0], "TAG"), "NEW");
    assert_eq!(attr(&s, refs[1], "TAG"), "ONE");
    s.execute("undo", &json!({})).unwrap();
    assert_eq!(attr(&s, refs[1], "TAG"), "NEW");
    // Typed: pick the block, the tag, the text.
    s.cmdline("gatte").unwrap();
    s.input(cadaddy_engine::Input::Pick(vec![refs[0]])).unwrap();
    s.cmdline("TAG").unwrap();
    s.cmdline("TYPED").unwrap();
    assert!(s.running.is_none());
    assert_eq!(attr(&s, refs[1], "TAG"), "TYPED");
    for bad in [
        json!({}),
        json!({"block": "NOPE", "tag": "TAG", "value": "x"}),
        json!({"block": "OUTER", "tag": "NOPE", "value": "x"}),
        json!({"block": "OUTER", "tag": "TAG"}),
    ] {
        assert!(s.execute("gatte", &bad).is_err(), "{bad}");
    }
}

#[test]
fn getsel_and_ssx_select_by_layer_type_and_properties() {
    let mut s = Session::new();
    s.execute("layer.new", &json!({"name": "Walls"})).unwrap();
    s.execute("line", &json!({"points": [[0, 0], [10, 0]]})).unwrap();
    let a = last(&s);
    s.execute("line", &json!({"points": [[0, 5], [10, 5]]})).unwrap();
    let b = last(&s);
    s.execute("circle", &json!({"center": [20, 0], "radius": 2})).unwrap();
    let c = last(&s);
    s.execute("properties.set", &json!({"handles": [b.hex(), c.hex()], "layer": "Walls"})).unwrap();
    let r = s.execute("getsel", &json!({"layer": "walls"})).unwrap();
    assert_eq!(r["selected"], 2);
    let r = s.execute("getsel", &json!({"layer": "W*", "type": "line"})).unwrap();
    assert_eq!(r["selected"], 1);
    assert_eq!(s.selection(), vec![b]);
    s.execute("getsel", &json!({"type": "LINE"})).unwrap();
    assert_eq!(s.selection().len(), 2);
    // Typed: layer name, then Enter for any type.
    s.cmdline("getsel").unwrap();
    s.cmdline("0").unwrap();
    s.cmdline("").unwrap();
    assert_eq!(s.selection(), vec![a]);

    let r = s.execute("ssx", &json!({"handle": b.hex()})).unwrap();
    assert_eq!(r["selected"], 1, "only line on Walls");
    s.execute("ssx", &json!({"handle": b.hex(), "match": ["layer"]})).unwrap();
    assert_eq!(s.selection().len(), 2);
    s.execute("ssx", &json!({"handle": a.hex(), "match": ["type"]})).unwrap();
    assert_eq!(s.selection().len(), 2);
    assert!(s.execute("ssx", &json!({"handle": a.hex(), "match": ["size"]})).is_err());
    s.set_selection(Vec::new());
    assert!(s.execute("ssx", &json!({})).is_err());
}

#[test]
fn xdlist_xlist_and_showurls_report() {
    let (mut s, refs) = blocks();
    s.doc_mut()
        .unwrap()
        .modify_entity(refs[2], |e| {
            e.common.xdata = vec![
                RawTag { code: 1001, value: "MYAPP".into() },
                RawTag { code: 1000, value: "hello".into() },
                RawTag { code: 1070, value: "7".into() },
                RawTag { code: 1001, value: "OTHER".into() },
                RawTag { code: 1040, value: "2.5".into() },
            ]
        })
        .unwrap();
    let r = s.execute("xdlist", &json!({"handle": refs[2].hex()})).unwrap();
    assert_eq!(r["apps"][0]["app"], "MYAPP");
    assert_eq!(r["apps"][0]["tags"][1], json!({"code": 1070, "value": "7"}));
    assert_eq!(r["apps"][1]["app"], "OTHER");
    let r = s.execute("xdlist", &json!({"handle": refs[2].hex(), "app": "oth*"})).unwrap();
    assert_eq!(r["apps"].as_array().unwrap().len(), 1);
    let r = s.execute("xdlist", &json!({"handle": refs[0].hex()})).unwrap();
    assert_eq!(r["apps"], json!([]));

    // XLIST: the line of OUTER near (5, 0), a circle of the nested INNER near (2, 3).
    let r = s.execute("xlist", &json!({"handle": refs[0].hex(), "at": [5, 0.1]})).unwrap();
    assert_eq!((r["type"].as_str(), r["block"].as_str(), r["layer"].as_str()), (Some("LINE"), Some("OUTER"), Some("0")));
    let r = s.execute("xlist", &json!({"handle": refs[0].hex(), "at": [2, 3.05]})).unwrap();
    assert_eq!((r["type"].as_str(), r["block"].as_str()), (Some("CIRCLE"), Some("INNER")));
    s.execute("line", &json!({"points": [[0, 0], [1, 1]]})).unwrap();
    let l = last(&s);
    assert!(s.execute("xlist", &json!({"handle": l.hex()})).is_err());
    // Typed: a point on the reference.
    s.cmdline("xlist").unwrap();
    s.input(cadaddy_engine::Input::Point(cadaddy_engine::geom::Vec2::new(55.0, 0.0))).unwrap();
    assert!(s.log.join("\n").contains("Block name: OUTER"), "{}", s.log.join("\n"));

    let r = s.execute("showurls", &json!({})).unwrap();
    assert_eq!(r["links"], json!([]));
    s.execute("hyperlink", &json!({"handles": [refs[1].hex()], "url": "https://example.com/spec", "description": "Spec"})).unwrap();
    let r = s.execute("showurls", &json!({})).unwrap();
    assert_eq!(r["links"][0]["url"], "https://example.com/spec");
    assert_eq!(r["links"][0]["handle"], refs[1].hex());
}

#[test]
fn redir_replaces_a_folder_in_style_xref_and_rtext_paths() {
    let mut s = Session::new();
    {
        let d = s.doc_mut().unwrap();
        d.text_styles[0].font = "C:\\old\\fonts\\mine.ttf".into();
        d.text_styles[0].big_font = "plain.shx".into();
    }
    s.execute("circle", &json!({"center": [0, 0], "radius": 1})).unwrap();
    let c = last(&s);
    s.execute("block", &json!({"name": "XR", "base": [0, 0], "handles": [c.hex()], "keep": "delete"})).unwrap();
    {
        let d = s.doc_mut().unwrap();
        let b = d.blocks.get_mut("XR").unwrap();
        std::sync::Arc::make_mut(b).xref_path = Some("/old/plans/site.dwg".into());
    }
    let rt = h(&s.execute("rtext", &json!({"file": "/old/notes/a.txt", "at": [0, 0]})).unwrap());

    // REDIRMODE limits what REDIR touches.
    let r = s.execute("redirmode", &json!({"kinds": ["styles"]})).unwrap();
    assert_eq!(r["kinds"], json!(["styles"]));
    let r = s.execute("redir", &json!({"from": "c:/OLD", "to": "D:\\new"})).unwrap();
    assert_eq!(r["changed"].as_array().unwrap().len(), 1);
    assert_eq!(s.doc().unwrap().text_styles[0].font, "D:\\new\\fonts\\mine.ttf", "subfolders kept, separators kept");
    assert_eq!(s.doc().unwrap().text_styles[0].big_font, "plain.shx", "no folder: left alone");
    assert_eq!(s.doc().unwrap().block("XR").unwrap().xref_path.as_deref(), Some("/old/plans/site.dwg"));

    let r = s.execute("redir", &json!({"from": "/old", "to": "/srv/new", "kinds": ["xrefs", "rtext"]})).unwrap();
    assert_eq!(r["changed"].as_array().unwrap().len(), 2);
    assert_eq!(s.doc().unwrap().block("XR").unwrap().xref_path.as_deref(), Some("/srv/new/plans/site.dwg"));
    let e = s.doc().unwrap().entity(rt).unwrap().clone();
    let src: Vec<&str> = e.common.xdata.iter().filter(|t| t.code == 1000).map(|t| t.value.as_str()).collect();
    assert_eq!(src.concat(), "/srv/new/notes/a.txt");
    // `*`: any folder.
    s.execute("redir", &json!({"from": "*", "to": "/flat", "kinds": ["xrefs"]})).unwrap();
    assert_eq!(s.doc().unwrap().block("XR").unwrap().xref_path.as_deref(), Some("/flat/site.dwg"));
    // Typed REDIRMODE and REDIR.
    s.cmdline("redirmode").unwrap();
    s.cmdline("xrefs").unwrap();
    assert_eq!(s.settings.redirmode, vec!["xrefs".to_string()]);
    s.cmdline("redir").unwrap();
    s.cmdline("/flat").unwrap();
    s.cmdline("/typed").unwrap();
    assert_eq!(s.doc().unwrap().block("XR").unwrap().xref_path.as_deref(), Some("/typed/site.dwg"));
    assert!(s.execute("redir", &json!({"to": "/x"})).is_err());
    assert!(s.execute("redir", &json!({"from": "/x", "to": "/y", "kinds": ["fonts"]})).is_err());
    assert!(s.execute("redirmode", &json!({"kinds": 5})).is_err());
}

#[test]
fn mpedit_edits_several_polylines() {
    let mut s = Session::new();
    s.execute("pline", &json!({"points": [[0, 0], [10, 0], [10, 10]]})).unwrap();
    let a = last(&s);
    s.execute("pline", &json!({"points": [[20, 0], [30, 0], [30, 10]]})).unwrap();
    let b = last(&s);
    s.execute("mpedit", &json!({"handles": hexes(&[a, b]), "option": "close"})).unwrap();
    for x in [a, b] {
        match &s.doc().unwrap().entity(x).unwrap().kind {
            EntityKind::LwPolyline(p) => assert!(p.closed),
            k => panic!("{k:?}"),
        }
    }
    // Typed: straight into "Select objects", then the options.
    s.cmdline("mpedit").unwrap();
    assert!(s.prompt_text().contains("Select objects"), "{}", s.prompt_text());
    s.input(cadaddy_engine::Input::Pick(vec![a, b])).unwrap();
    s.cmdline("").unwrap();
    s.cmdline("open").unwrap();
    s.cmdline("").unwrap();
    for x in [a, b] {
        match &s.doc().unwrap().entity(x).unwrap().kind {
            EntityKind::LwPolyline(p) => assert!(!p.closed),
            k => panic!("{k:?}"),
        }
    }
}

#[test]
fn lsp_lists_what_lisp_defined_and_lspsurf_reads_a_file() {
    let mut s = Session::new();
    let r = s.execute("lsp", &json!({})).unwrap();
    assert_eq!(r["commands"], json!([]));
    assert_eq!(r["functions"], json!([]));
    s.execute("lisp", &json!({"code": "(defun c:hello () (princ)) (defun add2 (a b / c) (+ a b)) (setq myvar 42)"})).unwrap();
    let r = s.execute("lsp", &json!({})).unwrap();
    assert_eq!(r["commands"], json!(["HELLO"]));
    assert_eq!(r["functions"], json!(["ADD2"]));
    assert_eq!(r["variables"], json!([{"name": "MYVAR", "value": "42"}]));
    let r = s.execute("lsp", &json!({"kind": "functions", "pattern": "x*"})).unwrap();
    assert_eq!(r["functions"], json!([]));
    assert!(r.get("commands").is_none());
    assert!(s.execute("lsp", &json!({"kind": "macros"})).is_err());
    s.cmdline("lsp").unwrap();
    s.cmdline("c").unwrap();
    assert!(s.log.join("\n").contains("Commands: HELLO"), "{}", s.log.join("\n"));

    let src = ";; tools\n(defun c:one () (princ))\n\n(progn\n  (defun helper (x y / tmp) (* x y)))\n";
    let r = s.execute("lspsurf", &json!({"text": src})).unwrap();
    assert_eq!(r["functions"], json!([{"name": "C:ONE", "params": [], "line": 2}, {"name": "HELPER", "params": ["X", "Y"], "line": 5}]));
    let r = s.execute("lspsurf", &json!({"text": src, "pattern": "c:*"})).unwrap();
    assert_eq!(r["functions"].as_array().unwrap().len(), 1);
    let dir = std::env::temp_dir().join(format!("cadaddy-lspsurf-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("tools.lsp");
    std::fs::write(&path, src).unwrap();
    let r = s.execute("lspsurf", &json!({"path": path.to_string_lossy()})).unwrap();
    assert_eq!(r["functions"].as_array().unwrap().len(), 2);
    std::fs::remove_dir_all(&dir).ok();
    assert!(s.execute("lspsurf", &json!({})).is_err());
    assert!(s.execute("lspsurf", &json!({"text": "(defun broken ("})).is_err());
    assert!(s.execute("lspsurf", &json!({"path": "/no/such/file.lsp"})).is_err());
}

#[test]
fn revert_goes_back_to_the_saved_drawing() {
    use cadaddy_engine::cmd::file::{IoHooks, set_io};
    set_io(IoHooks { read: |_, _| Ok(cadaddy_engine::doc::Drawing::new_metric()), write: |_, _| Ok(b"saved".to_vec()), plot: None, read_pdf: None });
    let mut s = Session::new();
    assert!(s.execute("revert", &json!({})).is_err(), "never saved");
    s.execute("circle", &json!({"center": [0, 0], "radius": 1})).unwrap();
    let dir = std::env::temp_dir().join(format!("cadaddy-revert-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("r.dxf");
    s.execute("saveas", &json!({"path": path.to_string_lossy()})).unwrap();
    let r = s.execute("revert", &json!({})).unwrap();
    assert_eq!(r["reverted"], false);
    s.execute("line", &json!({"points": [[0, 0], [5, 5]]})).unwrap();
    s.execute("line", &json!({"points": [[0, 5], [5, 0]]})).unwrap();
    assert_eq!(s.doc().unwrap().model.len(), 3);
    let r = s.execute("revert", &json!({})).unwrap();
    assert_eq!(r["reverted"], true);
    assert_eq!(s.doc().unwrap().model.len(), 1);
    assert!(!s.state().unwrap().is_dirty());
    s.execute("undo", &json!({})).unwrap();
    assert_eq!(s.doc().unwrap().model.len(), 3, "Undo brings the changes back");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn tframes_toggles_image_and_wipeout_frames() {
    let mut s = Session::new();
    s.execute("setvar", &json!({"name": "IMAGEFRAME", "value": 1})).unwrap();
    let r = s.execute("tframes", &json!({})).unwrap();
    assert_eq!((r["imageframe"].as_i64(), r["wipeoutframe"].as_i64()), (Some(0), Some(0)));
    let r = s.execute("tframes", &json!({})).unwrap();
    assert_eq!((r["imageframe"].as_i64(), r["wipeoutframe"].as_i64()), (Some(1), Some(1)));
    let v = s.execute("getvar", &json!({"name": "WIPEOUTFRAME"})).unwrap();
    assert_eq!(v.as_i64().or_else(|| v["value"].as_i64()), Some(1));
    s.execute("tframes", &json!({"on": false})).unwrap();
    let v = s.execute("getvar", &json!({"name": "IMAGEFRAME"})).unwrap();
    assert_eq!(v.as_i64().or_else(|| v["value"].as_i64()), Some(0));
    assert!(s.execute("tframes", &json!({"on": "maybe"})).is_err());
}

#[test]
fn bscale_and_brotate_turn_references_about_their_insertion_points() {
    let (mut s, refs) = blocks();
    let before = insert(&s, refs[1]);
    let tag_at = before.attribs[0].text.insert;
    s.execute("bscale", &json!({"handles": [refs[1].hex()], "factor": 2})).unwrap();
    let i = insert(&s, refs[1]);
    assert_eq!((i.scale.x, i.scale.y, i.insert), (2.0, 2.0, before.insert));
    let moved = i.attribs[0].text.insert;
    assert!(
        (moved.x - (50.0 + (tag_at.x - 50.0) * 2.0)).abs() < 1e-9 && (moved.y - tag_at.y * 2.0).abs() < 1e-9,
        "the attribute moved with it: {moved:?}"
    );
    assert!((i.attribs[0].text.height - before.attribs[0].text.height * 2.0).abs() < 1e-9);
    s.execute("bscale", &json!({"handles": [refs[1].hex()], "factor": 1.5, "relative": true})).unwrap();
    assert_eq!(insert(&s, refs[1]).scale.x, 3.0);

    s.execute("brotate", &json!({"handles": [refs[0].hex()], "angle": 90})).unwrap();
    let i = insert(&s, refs[0]);
    assert!((i.rotation - std::f64::consts::FRAC_PI_2).abs() < 1e-12);
    // The attribute at (0, -2) is now at (2, 0).
    let p = i.attribs[0].text.insert;
    assert!((p.x - 2.0).abs() < 1e-9 && p.y.abs() < 1e-9, "{p:?}");
    s.execute("brotate", &json!({"handles": [refs[0].hex()], "angle": 90, "relative": true})).unwrap();
    assert!((insert(&s, refs[0]).rotation - std::f64::consts::PI).abs() < 1e-12);
    // Typed: absolute, then relative.
    s.set_selection(Vec::new());
    s.cmdline("brotate").unwrap();
    s.input(cadaddy_engine::Input::Pick(vec![refs[2]])).unwrap();
    s.cmdline("").unwrap();
    s.cmdline("r").unwrap();
    s.cmdline("45").unwrap();
    assert!(s.running.is_none());
    assert!((insert(&s, refs[2]).rotation - std::f64::consts::FRAC_PI_4).abs() < 1e-12);

    s.execute("line", &json!({"points": [[0, 0], [1, 1]]})).unwrap();
    let l = last(&s);
    assert!(s.execute("bscale", &json!({"handles": [l.hex()], "factor": 2})).is_err(), "not a block reference");
    for f in [json!(0), json!(-1), json!("x"), json!(f64::NAN)] {
        assert!(s.execute("bscale", &json!({"handles": [refs[0].hex()], "factor": f})).is_err(), "{f}");
    }
    assert!(s.execute("brotate", &json!({"handles": [refs[0].hex()]})).is_err());
}

#[test]
fn vpsync_lines_viewports_up_with_the_master() {
    let mut s = Session::new();
    s.execute("layout.set", &json!({"name": "Layout1"})).unwrap();
    let vh = |r: Value| Handle::parse_hex(r["viewports"][0].as_str().unwrap()).unwrap();
    let a = vh(s.execute("mview", &json!({"p1": [0, 0], "p2": [100, 100]})).unwrap());
    let b = vh(s.execute("mview", &json!({"p1": [150, 0], "p2": [250, 50]})).unwrap());
    let vp = |s: &Session, x: Handle| match &s.doc().unwrap().entity(x).unwrap().kind {
        EntityKind::Viewport(v) => v.clone(),
        k => panic!("{k:?}"),
    };
    s.doc_mut()
        .unwrap()
        .modify_entity(a, |e| {
            if let EntityKind::Viewport(v) = &mut e.kind {
                v.view_center = cadaddy_engine::geom::Vec2::new(1000.0, 500.0);
                v.view_height = 200.0;
            }
        })
        .unwrap();
    s.execute("vpsync", &json!({"master": a.hex(), "handles": [b.hex()]})).unwrap();
    let (m, v) = (vp(&s, a), vp(&s, b));
    // Scale 0.5 (paper per model): the slave's height 50 shows 100; its centre is 150 paper
    // units right and 25 down from the master's, so 300 and 50 in model space.
    assert!((v.view_height - 100.0).abs() < 1e-9, "{}", v.view_height);
    let want = m.view_center + (v.center.xy() - m.center.xy()) * 2.0;
    assert!((v.view_center - want).len() < 1e-9, "{:?} vs {want:?}", v.view_center);
    s.cmdline("vpsync").unwrap();
    s.input(cadaddy_engine::Input::Pick(vec![a])).unwrap();
    s.input(cadaddy_engine::Input::Pick(vec![b])).unwrap();
    s.cmdline("").unwrap();
    assert!(s.running.is_none());
    assert!(s.execute("vpsync", &json!({"master": a.hex(), "handles": [a.hex()]})).is_err());
    assert!(s.execute("vpsync", &json!({"handles": [b.hex()]})).is_err());
    s.execute("line", &json!({"points": [[0, 0], [1, 1]]})).unwrap();
    let l = s.doc().unwrap().layouts.iter().find(|l| l.name == "Layout1").unwrap().entities.iter().last().unwrap().handle;
    assert!(s.execute("vpsync", &json!({"master": l.hex(), "handles": [b.hex()]})).is_err());
}

#[test]
fn dimreassoc_brings_back_the_measured_text() {
    let mut s = Session::new();
    let d1 = h(&s.execute("dimlinear", &json!({"p1": [0, 0], "p2": [10, 0], "at": [5, 5]})).unwrap());
    let d2 = h(&s.execute("dimlinear", &json!({"p1": [0, 0], "p2": [0, 10], "at": [-5, 5]})).unwrap());
    for (x, t) in [(d1, "OVERRIDE"), (d2, "<>")] {
        s.doc_mut()
            .unwrap()
            .modify_entity(x, |e| {
                if let EntityKind::Dimension(d) = &mut e.kind {
                    d.text = t.into();
                }
            })
            .unwrap();
    }
    let r = s.execute("dimreassoc", &json!({"handles": hexes(&[d1, d2])})).unwrap();
    assert_eq!(r["handles"], json!([d1.hex()]), "only the override changes");
    match &s.doc().unwrap().entity(d1).unwrap().kind {
        EntityKind::Dimension(d) => assert_eq!(d.text, ""),
        k => panic!("{k:?}"),
    }
    s.execute("undo", &json!({})).unwrap();
    match &s.doc().unwrap().entity(d1).unwrap().kind {
        EntityKind::Dimension(d) => assert_eq!(d.text, "OVERRIDE"),
        k => panic!("{k:?}"),
    }
}

#[test]
fn hostile_params_are_refused_not_panicking() {
    let mut s = Session::new();
    for (cmd, p) in [
        ("bcount", json!({"handles": 5})),
        ("gatte", json!({"block": 1, "tag": [], "value": {}})),
        ("getsel", json!({"layer": 5, "type": null})),
        ("ssx", json!({"handle": "FFFFFF"})),
        ("xdlist", json!({"handle": "FFFFFF"})),
        ("xlist", json!({"handle": "FFFFFF", "at": "x"})),
        ("redir", json!({"from": "", "to": ""})),
        ("redirmode", json!({"kinds": [1, 2]})),
        ("mpedit", json!({"handles": [], "option": "close"})),
        ("lsp", json!({"kind": 3})),
        ("lspsurf", json!({"text": "((((((((((("})),
        ("bscale", json!({"handles": ["FFFFFF"], "factor": 1e308})),
        ("brotate", json!({"handles": ["FFFFFF"], "angle": "nan"})),
        ("vpsync", json!({"master": "FFFFFF", "handles": ["FFFFFF"]})),
        ("dimreassoc", json!({"handles": ["x"]})),
    ] {
        let _ = s.execute(cmd, &p);
    }
    assert!(s.running.is_none(), "JSON calls never prompt");
}
