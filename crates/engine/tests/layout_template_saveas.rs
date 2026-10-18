//! LAYOUT Template brings layouts in from a template drawing with what they use, keeping the
//! drawing's own definitions of names it already has; LAYOUT SAveas writes a layout out as a
//! template, and Template brings it back.

use std::collections::HashSet;
use std::sync::Arc;

use cadaddy_engine::Session;
use cadaddy_engine::cmd::file::{IoHooks, set_io};
use cadaddy_engine::doc::color::Color;
use cadaddy_engine::doc::geom::{Vec2, Vec3};
use cadaddy_engine::doc::{Block, Circle, Drawing, Entity, EntityKind, Handle, Insert, Layer, Layout, Line, Viewport};
use serde_json::json;

fn hooks() {
    set_io(IoHooks {
        read: |b, name| cadaddy_io::read(b, name).map_err(|e| e.to_string()),
        write: |d, name| cadaddy_io::write(d, name).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
    });
}

/// A fresh folder for one test.
fn dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-layouttpl-{name}-{}", std::process::id()));
    std::fs::remove_dir_all(&d).ok();
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn layouts(s: &Session) -> Vec<String> {
    let mut ls: Vec<_> = s.doc().unwrap().layouts.iter().map(|l| (l.tab_order, l.name.clone())).collect();
    ls.sort();
    ls.into_iter().map(|(_, n)| n).collect()
}

fn layer(name: &str, color: u8) -> Layer {
    let mut l = Layer::new(name);
    l.color = Color::Index(color);
    l
}

fn on(layer: &str, h: u64, kind: EntityKind) -> Entity {
    let mut e = Entity::new(Handle(h), kind);
    e.common.layer = layer.into();
    e
}

fn insert(block: &str, at: Vec3) -> EntityKind {
    EntityKind::Insert(Insert {
        block: block.into(),
        insert: at,
        scale: Vec3::new(1.0, 1.0, 1.0),
        rotation: 0.0,
        attribs: Vec::new(),
        cols: 1,
        rows: 1,
        col_spacing: 0.0,
        row_spacing: 0.0,
        clip: None,
    })
}

/// A template: layouts "Sheet A" (a title block reference on TB, a circle on BORDER clipping a
/// viewport, on an ANSI B page) and "Detail" (a line on TB). Block TITLE is a circle; layer TB is
/// red.
fn template() -> Drawing {
    let mut d = Drawing::new_imperial();
    d.layers.push(layer("TB", 1));
    d.layers.push(layer("BORDER", 3));
    d.layers.push(layer("HIDDEN-IN-VP", 4));
    let mut title = Block::new("TITLE");
    title.entities.push(Entity::new(Handle(0x50), EntityKind::Circle(Circle { center: Vec3::ZERO, radius: 1.0 })));
    d.blocks.insert("TITLE".into(), Arc::new(title));
    let mut sheet = Layout::new("Sheet A", 1);
    sheet.page.paper = "ANSI B".into();
    sheet.page.width_mm = 279.4;
    sheet.page.height_mm = 431.8;
    sheet.entities.push(on("TB", 0x200, insert("TITLE", Vec3::new(10.0, 1.0, 0.0))));
    sheet.entities.push(on("BORDER", 0x201, EntityKind::Circle(Circle { center: Vec3::new(5.0, 5.0, 0.0), radius: 3.0 })));
    let vp = Viewport {
        center: Vec3::new(5.0, 5.0, 0.0),
        width: 6.0,
        height: 6.0,
        view_center: Vec2::new(0.0, 0.0),
        view_height: 10.0,
        id: 2,
        frozen_layers: vec!["HIDDEN-IN-VP".into()],
        clip: Some(Handle(0x201)),
        ..Viewport::default()
    };
    sheet.entities.push(on("0", 0x202, EntityKind::Viewport(vp)));
    let mut detail = Layout::new("Detail", 2);
    detail.entities.push(on("TB", 0x210, EntityKind::Line(Line { a: Vec3::ZERO, b: Vec3::new(4.0, 0.0, 0.0) })));
    d.layouts = vec![sheet, detail];
    d.model.push(on("0", 0x300, EntityKind::Circle(Circle { center: Vec3::ZERO, radius: 2.0 })));
    d.handseed = 0x400;
    d
}

fn write_template(dir: &std::path::Path, name: &str) -> String {
    let path = dir.join(name);
    std::fs::write(&path, cadaddy_io::write(&template(), name).unwrap()).unwrap();
    path.to_string_lossy().to_string()
}

/// A drawing that already has layer TB (blue) and its own block TITLE (a line).
fn target() -> Session {
    let mut s = Session::new();
    let d = s.doc_mut().unwrap();
    d.layers.push(layer("TB", 5));
    let mut title = Block::new("TITLE");
    title.entities.push(Entity::new(Handle(0x90), EntityKind::Line(Line { a: Vec3::ZERO, b: Vec3::new(1.0, 0.0, 0.0) })));
    d.blocks.insert("TITLE".into(), Arc::new(title));
    s
}

fn all_handles(d: &Drawing) -> Vec<Handle> {
    let mut hs: Vec<Handle> = d.model.iter().map(|e| e.handle).collect();
    for l in &d.layouts {
        hs.extend(l.entities.iter().map(|e| e.handle));
    }
    for b in d.blocks.values() {
        hs.extend(b.entities.iter().map(|e| e.handle));
    }
    hs
}

#[test]
fn template_brings_a_layout_and_what_it_uses_keeping_existing_definitions() {
    hooks();
    let dir = dir("insert");
    let path = write_template(&dir, "office.dxf");
    let mut s = target();
    let r = s.execute("layout", &json!({ "option": "template", "path": path, "layouts": "sheet*" })).unwrap();
    assert_eq!(r["inserted"], json!([{ "from": "Sheet A", "to": "Sheet A" }]));
    assert_eq!(layouts(&s), ["Layout1", "Layout2", "Sheet A"]);

    let d = s.doc().unwrap();
    // Names the drawing has keep the drawing's definitions; the others come along.
    assert_eq!(d.layers.iter().find(|l| l.name == "TB").unwrap().color, Color::Index(5));
    assert!(matches!(d.block("TITLE").unwrap().entities.iter().next().unwrap().kind, EntityKind::Line(_)));
    assert_eq!(d.layers.iter().find(|l| l.name == "BORDER").unwrap().color, Color::Index(3));
    assert!(d.layers.iter().any(|l| l.name == "HIDDEN-IN-VP"), "a viewport's frozen layer comes along");
    // Not the Detail layout's or model space's objects.
    assert!(d.layout("Detail").is_none());

    let l = d.layout("Sheet A").unwrap();
    assert_eq!((l.page.paper.as_str(), l.page.width_mm), ("ANSI B", 279.4));
    assert_eq!(l.entities.iter().count(), 3);
    // Fresh handles, unique across the drawing.
    let hs = all_handles(d);
    assert_eq!(hs.iter().collect::<HashSet<_>>().len(), hs.len());
    assert!(hs.iter().all(|h| h.0 < d.handseed));
    // The viewport is clipped by the copied circle, and numbered after the drawing's own.
    let circle = l.entities.iter().find(|e| matches!(e.kind, EntityKind::Circle(_))).unwrap().handle;
    let vp = l.entities.iter().find_map(|e| if let EntityKind::Viewport(v) = &e.kind { (v.id != 1).then_some(v.clone()) } else { None }).unwrap();
    assert_eq!(vp.clip, Some(circle));
    let ids: Vec<u32> = d
        .layouts
        .iter()
        .flat_map(|l| l.entities.iter())
        .filter_map(|e| if let EntityKind::Viewport(v) = &e.kind { Some(v.id) } else { None })
        .collect();
    assert_eq!(ids.iter().collect::<HashSet<_>>().len(), ids.len(), "viewport ids {ids:?}");

    // One undo step.
    s.undo().unwrap();
    assert_eq!(layouts(&s), ["Layout1", "Layout2"]);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn template_names_that_are_taken_get_a_number() {
    hooks();
    let dir = dir("clash");
    let path = write_template(&dir, "office.dxf");
    let mut s = target();
    s.execute("layout", &json!({ "option": "template", "path": path })).unwrap();
    let r = s.execute("layout", &json!({ "option": "t", "path": path, "layouts": ["Sheet A", "DETAIL"] })).unwrap();
    assert_eq!(r["inserted"], json!([{ "from": "Sheet A", "to": "Sheet A (2)" }, { "from": "Detail", "to": "Detail (2)" }]));
    assert_eq!(layouts(&s), ["Layout1", "Layout2", "Sheet A", "Detail", "Sheet A (2)", "Detail (2)"]);
    // Nothing matching is an error.
    let e = s.execute("layout", &json!({ "option": "template", "path": path, "layouts": "Nope*" })).unwrap_err().to_string();
    assert!(e.contains("No matching layout"), "{e}");
    let e = s.execute("layout", &json!({ "option": "template", "path": dir.join("missing.dwt").to_string_lossy() })).unwrap_err().to_string();
    assert!(e.contains("Can't find file"), "{e}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn typed_template_asks_again_for_a_missing_file_or_layout() {
    hooks();
    let dir = dir("typed");
    let path = write_template(&dir, "office.dxf");
    let mut s = target();
    s.cmdline("-LAYOUT T").unwrap();
    assert_eq!(s.current_prompt().unwrap().display(), "Enter template file name:");
    let n = s.log.len();
    let _ = s.cmdline(&dir.join("nothere.dwt").to_string_lossy());
    assert_eq!(s.current_prompt().unwrap().display(), "Enter template file name:");
    assert!(s.log[n..].iter().any(|l| l.contains("Can't find file")), "{:?}", &s.log[n..]);
    // Without an extension, .dwt/.dwg/.dxf are tried.
    s.cmdline(path.trim_end_matches(".dxf")).unwrap();
    assert_eq!(s.current_prompt().unwrap().display(), "Enter layout name(s) to insert or [?]:");
    let n = s.log.len();
    s.cmdline("?").unwrap();
    assert_eq!(s.log[n..], ["Layout: Sheet A", "Layout: Detail"]);
    let n = s.log.len();
    let _ = s.cmdline("Elevations");
    assert!(s.log[n..].iter().any(|l| l.contains("No matching layout")), "{:?}", &s.log[n..]);
    assert!(s.current_prompt().is_some());
    s.cmdline("Sheet A, det*").unwrap();
    assert!(s.current_prompt().is_none());
    assert_eq!(layouts(&s), ["Layout1", "Layout2", "Sheet A", "Detail"]);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn saveas_then_template_brings_the_same_layout_back() {
    hooks();
    let dir = dir("roundtrip");
    let mut s = Session::new();
    *s.doc_mut().unwrap() = template();
    // Typed: the layout (Enter: the current one), then the file; no extension means .dwt.
    s.execute("layout.set", &json!({ "name": "Sheet A" })).unwrap();
    s.cmdline("-LAYOUT SA").unwrap();
    assert_eq!(s.current_prompt().unwrap().display(), "Enter layout to save to template <Sheet A>:");
    s.cmdline("").unwrap();
    let base = dir.join("sheet-a").to_string_lossy().to_string();
    s.cmdline(&base).unwrap();
    assert!(s.current_prompt().is_none());
    let dwt = format!("{base}.dwt");
    // JSON form, to DXF.
    let dxf = dir.join("sheet-a.dxf").to_string_lossy().to_string();
    let r = s.execute("layout", &json!({ "option": "saveas", "layout": "sheet a", "path": dxf })).unwrap();
    assert_eq!(r["layout"], "Sheet A");

    for file in [&dwt, &dxf] {
        // The template holds that layout only, model space, and what they use.
        let t = cadaddy_io::read(&std::fs::read(file).unwrap(), file).unwrap();
        let names: Vec<&str> = t.layouts.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, ["Sheet A"], "{file}");
        assert_eq!(t.model.iter().count(), 1, "{file}");
        assert!(t.block("TITLE").is_some() && t.layers.iter().any(|l| l.name == "BORDER"), "{file}");

        let mut back = Session::new();
        back.execute("layout", &json!({ "option": "template", "path": file, "layouts": "*" })).unwrap();
        let d = back.doc().unwrap();
        let src = template();
        let (a, b) = (src.layout("Sheet A").unwrap(), d.layout("Sheet A").unwrap());
        assert_eq!((b.page.paper.as_str(), b.page.width_mm, b.page.height_mm), (a.page.paper.as_str(), a.page.width_mm, a.page.height_mm), "{file}");
        let kinds = |l: &Layout| {
            let mut k: Vec<String> = l
                .entities
                .iter()
                .filter(|e| !matches!(&e.kind, EntityKind::Viewport(v) if v.id == 1))
                .map(|e| format!("{}:{}", e.kind.type_name(), e.common.layer))
                .collect();
            k.sort();
            k
        };
        assert_eq!(kinds(b), kinds(a), "{file}");
        let vp = b.entities.iter().find_map(|e| if let EntityKind::Viewport(v) = &e.kind { (v.id != 1).then_some(v.clone()) } else { None }).unwrap();
        assert!(vp.clip.is_some_and(|h| b.entities.iter().any(|e| e.handle == h && matches!(e.kind, EntityKind::Circle(_)))), "{file}");
        assert!(matches!(d.block("TITLE").unwrap().entities.iter().next().unwrap().kind, EntityKind::Circle(_)), "{file}");
    }

    // Saving needs a layout that exists.
    let e = s.execute("layout", &json!({ "option": "saveas", "layout": "Nope", "path": dxf })).unwrap_err().to_string();
    assert!(e.contains("no layout"), "{e}");
    std::fs::remove_dir_all(&dir).ok();
}
