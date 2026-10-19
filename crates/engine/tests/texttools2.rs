//! Text tools: SCALETEXT, JUSTIFYTEXT/TJUST, TORIENT, TEXTMASK/TEXTUNMASK, TCIRCLE, TEXTFIT,
//! TFRAME, RTEXT (with DIESEL) and ATTOUT/ATTIN, each by its JSON form and typed.

use cadaddy_doc::{EntityKind, Handle, Text};
use cadaddy_engine::Session;
use cadaddy_geom::{Bounds2, Vec2};
use serde_json::{Value, json};

fn handle(r: &Value) -> Handle {
    Handle::parse_hex(r["handle"].as_str().or_else(|| r["handles"][0].as_str()).unwrap()).unwrap()
}

fn text(s: &Session, h: Handle) -> Text {
    match &s.doc().unwrap().entity(h).unwrap().kind {
        EntityKind::Text(t) => t.clone(),
        other => panic!("not text: {other:?}"),
    }
}

/// Where the text's glyphs are, as the renderer draws them.
fn glyph_box(s: &Session, h: Handle) -> Bounds2 {
    let t = text(s, h);
    let sh = cadaddy_render::place_text_entity(s.doc().unwrap(), &t, &t.value).0;
    Bounds2::from_points(sh.strokes.iter().flatten().copied().chain(sh.glyphs.iter().flatten().flatten().copied()))
}

fn same_box(a: Bounds2, b: Bounds2, tol: f64) -> bool {
    a.min.dist(b.min) < tol && a.max.dist(b.max) < tol
}

fn add_text(s: &mut Session, at: [f64; 2], value: &str, height: f64, rotation: f64, justify: &str) -> Handle {
    handle(&s.execute("text", &json!({"at": at, "text": value, "height": height, "rotation": rotation, "justify": justify})).unwrap())
}

#[test]
fn justifytext_keeps_the_text_where_it_is() {
    let mut s = Session::new();
    for rot in [0.0, 30.0, 200.0] {
        let h = add_text(&mut s, [10.0, 5.0], "Panel LP-1", 2.5, rot, "L");
        let before = glyph_box(&s, h);
        for j in ["tl", "tc", "tr", "ml", "mc", "mr", "bl", "bc", "br", "center", "middle", "right", "left"] {
            s.execute("justifytext", &json!({"handles": [h.hex()], "justify": j})).unwrap();
            let after = glyph_box(&s, h);
            assert!(same_box(before, after, 1e-6), "{rot}° {j}: {before:?} → {after:?}");
        }
        // Aligned and fit span the same baseline.
        for j in ["align", "fit", "left"] {
            s.execute("justifytext", &json!({"handles": [h.hex()], "justify": j})).unwrap();
            assert!(same_box(before, glyph_box(&s, h), 1e-6), "{rot}° {j}");
        }
    }
    let x = add_text(&mut s, [0.0, 0.0], "x", 1.0, 0.0, "L");
    let t = text(&s, x);
    assert_eq!(t.halign, cadaddy_doc::HAlign::Left);
    assert!(s.execute("justifytext", &json!({"handles": [], "justify": "tl"})).is_err());
    assert!(s.execute("justifytext", &json!({"justify": "sideways"})).is_err());
    // TJUST is the same command.
    let h = add_text(&mut s, [0.0, 0.0], "abc", 1.0, 0.0, "L");
    s.set_selection(vec![h]);
    s.script("TJUST MC\n").unwrap();
    assert_eq!(text(&s, h).valign, cadaddy_doc::VAlign::Middle);
}

#[test]
fn justifytext_moves_an_mtext_attachment_not_the_text() {
    let mut s = Session::new();
    let r = s.execute("mtext", &json!({"at": [0, 10], "text": "Line one\\PLine two", "height": 2.0})).unwrap();
    let h = handle(&r);
    let laid = |s: &Session| match &s.doc().unwrap().entity(h).unwrap().kind {
        EntityKind::MText(m) => (m.attach, cadaddy_render::layout_mtext_entity(s.doc().unwrap(), m).bounds),
        _ => panic!(),
    };
    let (_, before) = laid(&s);
    s.execute("justifytext", &json!({"handles": [h.hex()], "justify": "br"})).unwrap();
    let (attach, after) = laid(&s);
    assert_eq!(attach, 9);
    assert!(same_box(before, after, 1e-6), "{before:?} {after:?}");
}

#[test]
fn scaletext_scales_about_the_chosen_point() {
    let mut s = Session::new();
    let h = add_text(&mut s, [10.0, 5.0], "DATA", 2.0, 0.0, "MC");
    s.execute("scaletext", &json!({"handles": [h.hex()], "height": 4.0})).unwrap();
    let t = text(&s, h);
    assert!((t.height - 4.0).abs() < 1e-12);
    assert!(t.justify_point().dist(Vec2::new(10.0, 5.0)) < 1e-9, "existing: the MC point stays");
    // About the top left corner: it stays put.
    let tl = |s: &Session| {
        let b = glyph_box(s, h);
        Vec2::new(b.min.x, b.max.y)
    };
    let before = tl(&s);
    s.execute("scaletext", &json!({"handles": [h.hex()], "factor": 0.5, "base": "tl"})).unwrap();
    assert!((text(&s, h).height - 2.0).abs() < 1e-12);
    // The glyphs' own top left moves with their shapes; the frame's top left is exact.
    assert!(tl(&s).dist(before) < 0.3, "{:?} {:?}", tl(&s), before);
    // Match another text's height.
    let other = add_text(&mut s, [0.0, 0.0], "x", 7.0, 0.0, "L");
    s.execute("scaletext", &json!({"handles": [h.hex()], "match": other.hex()})).unwrap();
    assert!((text(&s, h).height - 7.0).abs() < 1e-12);
    assert!(s.execute("scaletext", &json!({"handles": [h.hex()], "height": -1})).is_err());
    assert!(s.execute("scaletext", &json!({"handles": [h.hex()]})).is_err());
    // Typed: base option, then height.
    s.set_selection(vec![h]);
    s.script("SCALETEXT E 3\n").unwrap();
    assert!((text(&s, h).height - 3.0).abs() < 1e-12, "{}", text(&s, h).height);
    s.set_selection(vec![h]);
    s.script("SCALETEXT  S 2\n").unwrap();
    assert!((text(&s, h).height - 6.0).abs() < 1e-12);
}

#[test]
fn torient_turns_upside_down_text_over_in_place() {
    let mut s = Session::new();
    let a = add_text(&mut s, [0.0, 0.0], "UPSIDE", 2.0, 180.0, "L");
    let b = add_text(&mut s, [0.0, 20.0], "FINE", 2.0, 45.0, "L");
    let center = |s: &Session, h| glyph_box(s, h).center();
    let (ca, cb) = (center(&s, a), center(&s, b));
    s.execute("torient", &json!({"handles": [a.hex(), b.hex()]})).unwrap();
    assert!(text(&s, a).rotation.abs() < 1e-9, "{}", text(&s, a).rotation);
    assert!(center(&s, a).dist(ca) < 1e-6);
    assert!((text(&s, b).rotation - 45f64.to_radians()).abs() < 1e-9, "readable text stays");
    assert!(center(&s, b).dist(cb) < 1e-6);
    s.execute("torient", &json!({"handles": [b.hex()], "angle": 90})).unwrap();
    assert!((text(&s, b).rotation - 90f64.to_radians()).abs() < 1e-9);
    // Typed: Enter takes the most readable.
    let c = add_text(&mut s, [5.0, 5.0], "TYPED", 1.0, 260.0, "L");
    s.set_selection(vec![c]);
    s.script("TORIENT \n").unwrap();
    assert!((text(&s, c).rotation - 80f64.to_radians()).abs() < 1e-9, "{}", text(&s, c).rotation.to_degrees());
}

#[test]
fn textmask_puts_a_wipeout_under_the_text_and_textunmask_takes_it_away() {
    let mut s = Session::new();
    let h = add_text(&mut s, [0.0, 0.0], "MASKED", 2.0, 0.0, "L");
    let r = s.execute("textmask", &json!({"handles": [h.hex()]})).unwrap();
    let m = handle(&json!({"handle": r["masks"][0]}));
    let order: Vec<Handle> = s.doc().unwrap().model.iter().map(|e| e.handle).collect();
    let (pm, pt) = (order.iter().position(|x| *x == m).unwrap(), order.iter().position(|x| *x == h).unwrap());
    assert!(pm < pt, "the mask draws first");
    let EntityKind::Wipeout(w) = &s.doc().unwrap().entity(m).unwrap().kind else { panic!("not a wipeout") };
    let wb = Bounds2::from_points(w.boundary.iter().copied());
    let tb = glyph_box(&s, h);
    assert!(wb.min.x < tb.min.x && wb.max.x > tb.max.x && wb.min.y < tb.min.y && wb.max.y > tb.max.y, "{wb:?} covers {tb:?}");
    // Masking again replaces the mask.
    s.execute("textmask", &json!({"handles": [h.hex()], "type": "solid", "offset": 0.5})).unwrap();
    let count = |s: &Session, f: fn(&EntityKind) -> bool| s.doc().unwrap().model.iter().filter(|e| f(&e.kind)).count();
    assert_eq!(count(&s, |k| matches!(k, EntityKind::Wipeout(_))), 0);
    assert_eq!(count(&s, |k| matches!(k, EntityKind::Solid(_))), 1);
    let r = s.execute("textunmask", &json!({"handles": [h.hex()]})).unwrap();
    assert_eq!(r["removed"], 1);
    assert_eq!(s.doc().unwrap().model.len(), 1);
    // MTEXT takes its own background mask.
    let mt = handle(&s.execute("mtext", &json!({"at": [0, 10], "text": "Note", "height": 2.0})).unwrap());
    s.execute("textmask", &json!({"handles": [mt.hex()]})).unwrap();
    assert!(matches!(&s.doc().unwrap().entity(mt).unwrap().kind, EntityKind::MText(m) if m.background.is_some()));
    s.execute("textunmask", &json!({"handles": [mt.hex()]})).unwrap();
    assert!(matches!(&s.doc().unwrap().entity(mt).unwrap().kind, EntityKind::MText(m) if m.background.is_none()));
    assert!(s.execute("textmask", &json!({"handles": [h.hex()], "offset": 50})).is_err());
}

#[test]
fn tcircle_encloses_each_text() {
    let mut s = Session::new();
    let a = add_text(&mut s, [0.0, 0.0], "A1", 2.0, 0.0, "L");
    let b = add_text(&mut s, [0.0, 10.0], "LONGER LABEL", 2.0, 0.0, "L");
    let r = s.execute("tcircle", &json!({"handles": [a.hex(), b.hex()]})).unwrap();
    assert_eq!(r["handles"].as_array().unwrap().len(), 2);
    let radii: Vec<f64> =
        s.doc().unwrap().model.iter().filter_map(|e| if let EntityKind::Circle(c) = &e.kind { Some(c.radius) } else { None }).collect();
    assert!(radii[0] < radii[1], "variable size: {radii:?}");
    let tb = glyph_box(&s, b);
    assert!(radii[1] > tb.width() / 2.0);
    s.execute("tcircle", &json!({"handles": [a.hex(), b.hex()], "shape": "rectangles", "size": "constant"})).unwrap();
    let rects: Vec<Bounds2> = s
        .doc()
        .unwrap()
        .model
        .iter()
        .filter_map(|e| if let EntityKind::LwPolyline(p) = &e.kind { Some(Bounds2::from_points(p.vertices.iter().map(|v| v.p))) } else { None })
        .collect();
    assert_eq!(rects.len(), 2);
    assert!((rects[0].width() - rects[1].width()).abs() < 1e-9, "constant size");
    s.execute("tcircle", &json!({"handles": [a.hex()], "shape": "slots"})).unwrap();
    let last = s.doc().unwrap().model.iter().last().unwrap().kind.clone();
    assert!(matches!(last, EntityKind::LwPolyline(p) if p.closed && p.vertices.iter().filter(|v| v.bulge != 0.0).count() == 2));
    // Typed: offset, shape, size.
    s.set_selection(vec![a]);
    s.script("TCIRCLE 0.5 R C\n").unwrap();
    assert!(s.running.is_none());
    assert!(s.execute("tcircle", &json!({"handles": [a.hex()], "offset": 3})).is_err());
}

#[test]
fn textfit_stretches_to_the_end_point() {
    let mut s = Session::new();
    let h = add_text(&mut s, [0.0, 0.0], "STRETCH", 2.0, 0.0, "L");
    let w = glyph_box(&s, h).width();
    let r = s.execute("textfit", &json!({"handle": h.hex(), "length": 30.0})).unwrap();
    let wf = r["widthFactor"].as_f64().unwrap();
    assert!(wf > 1.0);
    let w2 = glyph_box(&s, h).width();
    assert!((w2 / w - wf).abs() < 0.05, "{w} → {w2} with {wf}");
    s.execute("textfit", &json!({"handle": h.hex(), "end": [10.0, 3.0]})).unwrap();
    assert!(glyph_box(&s, h).max.x <= 10.5);
    assert!(s.execute("textfit", &json!({"handle": h.hex(), "end": [-5.0, 0.0]})).is_err());
    // Typed: pick the text, then the end point.
    s.set_selection(Vec::new());
    s.cmdline("textfit").unwrap();
    s.input(cadaddy_engine::Input::Pick(vec![h])).unwrap();
    s.cmdline("20,0").unwrap();
    assert!(s.running.is_none());
}

#[test]
fn tframe_toggles_image_and_wipeout_frames() {
    let mut s = Session::new();
    let r = s.execute("tframe", &json!({})).unwrap();
    let first = r["IMAGEFRAME"].as_i64().unwrap();
    let r = s.execute("tframe", &json!({})).unwrap();
    assert_eq!(r["IMAGEFRAME"].as_i64().unwrap(), 1 - first);
    assert_eq!(r["WIPEOUTFRAME"], r["IMAGEFRAME"]);
    let r = s.execute("tframe", &json!({"on": true})).unwrap();
    assert_eq!(r["WIPEOUTFRAME"], 1);
}

#[test]
fn rtext_shows_diesel_and_files_and_regen_updates_it() {
    let mut s = Session::new();
    let r =
        s.execute("rtext", &json!({"diesel": "$(+,1,2) and $(upper,$(substr,hello,2,3)) $(if,$(>,2,1),yes,no) $(eq,a,a)", "at": [0, 0]})).unwrap();
    let h = handle(&r);
    let contents = |s: &Session| match &s.doc().unwrap().entity(h).unwrap().kind {
        EntityKind::MText(m) => m.contents.clone(),
        _ => panic!(),
    };
    assert_eq!(contents(&s), "3 and ELL yes 1");
    let dir = std::env::temp_dir().join(format!("cadaddy-rtext-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("note.txt");
    std::fs::write(&f, "first\nsecond {x}\n").unwrap();
    let fh = handle(&s.execute("rtext", &json!({"file": f.display().to_string(), "at": [0, 10], "height": 3})).unwrap());
    let file_contents = |s: &Session| match &s.doc().unwrap().entity(fh).unwrap().kind {
        EntityKind::MText(m) => m.contents.clone(),
        _ => panic!(),
    };
    assert_eq!(file_contents(&s), "first\\Psecond \\{x\\}");
    std::fs::write(&f, "changed").unwrap();
    s.execute("regen", &json!({})).unwrap();
    assert_eq!(file_contents(&s), "changed");
    // The source survives a DXF round trip as xdata (checked through the codec-free path: the
    // xdata itself).
    let x = &s.doc().unwrap().entity(fh).unwrap().common.xdata;
    assert_eq!(x.first().map(|t| t.value.as_str()), Some("CADADDY_RTEXT"));
    assert!(s.execute("rtext", &json!({"at": [0, 0]})).is_err());
    assert!(s.execute("rtext", &json!({"diesel": "x"})).is_err());
    // Typed: Diesel, then the expression, then the point.
    s.script("RTEXT D $(*,6,7)\n5,5\n").unwrap();
    let last = s.doc().unwrap().model.iter().last().unwrap().kind.clone();
    assert!(matches!(&last, EntityKind::MText(m) if m.contents == "42"), "{last:?}");
}

#[test]
fn diesel_follows_its_rules() {
    let s = Session::new();
    let d = |e: &str| cadaddy_engine::cmd::express_diesel(&s, e);
    assert_eq!(d("plain text"), "plain text");
    assert_eq!(d("$(-,10,2,3)"), "5");
    assert_eq!(d("$(/,7,2)"), "3.5");
    assert_eq!(d("$(/,1,0)"), "$?");
    assert_eq!(d("$(nosuch,1)"), "$?");
    assert_eq!(d("$(+,1"), "$?", "unclosed");
    assert_eq!(d("$(strlen,\"a,b\")"), "3", "quotes keep commas");
    assert_eq!(d("$(index,1,\"a,b,c\")"), "b");
    assert_eq!(d("$(nth,2,x,y,z)"), "z");
    assert_eq!(d("$(and,12,10)"), "8");
    assert_eq!(d("$(fix,3.9)"), "3");
    assert_eq!(d("$(rtos,3.14159,2,2)"), "3.14");
    assert_eq!(d("$(eval,\"$(+,2,2)\")"), "4");
    assert_eq!(d("$(edtime,2451545.0,YYYY-MO-DD HH:MM DDDD MON)"), "2000-01-01 12:00 Saturday Jan");
    assert_eq!(d("$(edtime,2451545.0,H:MM AM/PM)"), "12:00 PM");
    assert!(!d("$(getvar,clayer)").contains('$'));
    // Deep nesting stays bounded.
    let deep = "$(+,1,".repeat(200) + "1" + &")".repeat(200);
    assert!(d(&deep).contains("$?"));
}

/// Block B with attributes T and U, and three references.
fn with_block() -> (Session, Vec<Handle>) {
    let mut s = Session::new();
    s.execute("attdef", &json!({"tag": "T", "default": "one", "at": [0, 1]})).unwrap();
    s.execute("attdef", &json!({"tag": "U", "default": "u", "at": [0, 2]})).unwrap();
    s.execute("line", &json!({"points": [[0, 0], [10, 0]]})).unwrap();
    let hs: Vec<String> = s.doc().unwrap().model.iter().map(|e| e.handle.hex()).collect();
    s.execute("block", &json!({"name": "B", "base": [0, 0], "handles": hs, "keep": "delete"})).unwrap();
    let refs = (0..3)
        .map(|k| handle(&s.execute("insert", &json!({"name": "B", "at": [f64::from(k) * 20.0, 0], "attribs": {"T": format!("t{k}")}})).unwrap()))
        .collect();
    (s, refs)
}

fn value(s: &Session, h: Handle, tag: &str) -> String {
    let EntityKind::Insert(i) = &s.doc().unwrap().entity(h).unwrap().kind else { panic!("not an insert") };
    i.attribs.iter().find(|a| a.tag == tag).unwrap().text.value.clone()
}

#[test]
fn attout_and_attin_round_trip_through_a_tab_separated_file() {
    let (mut s, refs) = with_block();
    let dir = std::env::temp_dir().join(format!("cadaddy-attio-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("atts.txt").display().to_string();
    let r = s.execute("attout", &json!({"path": path})).unwrap();
    assert_eq!(r["rows"], 3);
    let text = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], "HANDLE\tBLOCKNAME\tT\tU");
    assert_eq!(lines[1], format!("'{}\tB\tt0\tu", refs[0].hex()));
    // Edit in a "spreadsheet": change a value, leave one as <>, add a row for a missing handle.
    let edited =
        text.replace("\tt1\t", "\tCHANGED\t").replace(&format!("'{}\tB\tt2\tu", refs[2].hex()), &format!("'{}\tB\t<>\tnew u", refs[2].hex()))
            + "'FFFFF\tB\tx\ty\r\n";
    std::fs::write(&path, edited).unwrap();
    let r = s.execute("attin", &json!({"path": path})).unwrap();
    assert_eq!(r["updated"], 2, "{r}");
    assert_eq!(r["notFound"], 1);
    assert_eq!(value(&s, refs[1], "T"), "CHANGED");
    assert_eq!(value(&s, refs[2], "T"), "t2", "<> leaves it");
    assert_eq!(value(&s, refs[2], "U"), "new u");
    // A file without the header columns is refused.
    std::fs::write(&path, "A\tB\n1\t2\n").unwrap();
    assert!(s.execute("attin", &json!({"path": path})).is_err());
    assert!(s.execute("attin", &json!({"path": dir.join("missing.txt").display().to_string()})).is_err());
    // Only the selected references.
    let r = s.execute("attout", &json!({"path": path, "handles": [refs[0].hex()]})).unwrap();
    assert_eq!(r["rows"], 1);
}
