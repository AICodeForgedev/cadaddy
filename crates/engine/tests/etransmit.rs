//! ETRANSMIT: a drawing with nested xrefs and images packed into a ZIP archive or a folder, with
//! every path rewritten relative so the package opens anywhere.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cadaddy_engine::Session;
use cadaddy_engine::cmd::file::{IoHooks, set_io, zip};
use cadaddy_engine::doc::geom::Vec3;
use cadaddy_engine::doc::{Block, Drawing, Entity, EntityKind, Insert, Line, XrefStatus};
use serde_json::{Value, json};

fn hooks() {
    set_io(IoHooks {
        read: |b, name| cadaddy_io::read(b, name).map_err(|e| e.to_string()),
        write: |d, name| cadaddy_io::write(d, name).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
        read_dwf: None,
        write_dwfx: None,
    });
}

fn folder(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cadaddy-etransmit-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn save(d: &Drawing, path: &Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, cadaddy_io::write(d, &path.to_string_lossy()).unwrap()).unwrap();
}

fn line(d: &mut Drawing, y: f64) {
    let h = d.new_handle();
    d.model.push(Entity::new(h, EntityKind::Line(Line { a: Vec3::new(0.0, y, 0.0), b: Vec3::new(10.0, y, 0.0) })));
}

fn image(d: &mut Drawing, path: &str) {
    let im = serde_json::from_value(
        json!({ "insert": {"x":0,"y":0,"z":0}, "u": {"x":1,"y":0,"z":0}, "v": {"x":0,"y":1,"z":0}, "size": {"x":4,"y":4}, "path": path }),
    )
    .unwrap();
    let h = d.new_handle();
    d.model.push(Entity::new(h, EntityKind::Image(im)));
}

fn xref(d: &mut Drawing, name: &str, path: &str) {
    let mut b = Block::new(name);
    b.xref_path = Some(path.into());
    d.blocks.insert(name.into(), Arc::new(b));
    let h = d.new_handle();
    d.model.push(Entity::new(
        h,
        EntityKind::Insert(Insert {
            block: name.into(),
            insert: Vec3::ZERO,
            scale: Vec3::new(1.0, 1.0, 1.0),
            rotation: 0.0,
            attribs: Vec::new(),
            cols: 1,
            rows: 1,
            col_spacing: 0.0,
            row_spacing: 0.0,
            clip: None,
        }),
    ));
}

/// proj/host.dxf → parts/a.dxf (→ b.dxf beside it, images ../img/photo.png and other.png),
/// img/photo.png, and a missing gone.dxf.
fn project(tag: &str) -> (PathBuf, Session) {
    hooks();
    let dir = folder(tag);
    let mut b = Drawing::new_metric();
    line(&mut b, 0.0);
    save(&b, &dir.join("parts/b.dxf"));
    let mut a = Drawing::new_metric();
    line(&mut a, 1.0);
    xref(&mut a, "b", "b.dxf");
    image(&mut a, "../img/photo.png");
    image(&mut a, "../img/other.png");
    save(&a, &dir.join("parts/a.dxf"));
    std::fs::create_dir_all(dir.join("img")).unwrap();
    std::fs::write(dir.join("img/photo.png"), b"photo bytes").unwrap();
    std::fs::write(dir.join("img/other.png"), b"other bytes").unwrap();
    let mut host = Drawing::new_metric();
    xref(&mut host, "a", "parts/a.dxf");
    xref(&mut host, "gone", "gone.dxf");
    image(&mut host, "img/photo.png");
    save(&host, &dir.join("host.dxf"));
    let mut s = Session::new();
    s.execute("open", &json!({ "path": dir.join("host.dxf").to_string_lossy() })).unwrap();
    (dir, s)
}

fn entries(path: &Path) -> Vec<(String, Vec<u8>)> {
    zip::read(&std::fs::read(path).unwrap()).unwrap()
}

fn drawing(entries: &[(String, Vec<u8>)], name: &str) -> Drawing {
    let (_, b) = entries.iter().find(|(n, _)| n == name).unwrap_or_else(|| panic!("{name} not in the package"));
    cadaddy_io::read(b, name).unwrap()
}

fn image_paths(d: &Drawing) -> Vec<String> {
    let mut v: Vec<String> = d.model.iter().filter_map(|e| if let EntityKind::Image(i) = &e.kind { Some(i.path.clone()) } else { None }).collect();
    v.sort();
    v
}

#[test]
fn a_zip_holds_every_file_with_relative_paths() {
    let (dir, mut s) = project("zip");
    let out = dir.join("out.zip");
    let r = s.execute("etransmit", &json!({ "path": out.to_string_lossy() })).unwrap();
    let e = entries(&out);
    let mut names: Vec<&str> = e.iter().map(|(n, _)| n.as_str()).collect();
    names.sort();
    assert_eq!(names, ["Images/other.png", "Images/photo.png", "Xrefs/a.dxf", "Xrefs/b.dxf", "host.dxf", "transmittal.html", "transmittal.txt"]);
    // The host points into the package.
    let host = drawing(&e, "host.dxf");
    assert_eq!(host.block("a").unwrap().xref_path.as_deref(), Some("Xrefs\\a.dxf"));
    assert_eq!(image_paths(&host), ["Images\\photo.png"]);
    // The nested xref's own paths, relative to where it landed; the shared image is packed once.
    let a = drawing(&e, "Xrefs/a.dxf");
    assert_eq!(a.block("b").unwrap().xref_path.as_deref(), Some("b.dxf"));
    assert_eq!(image_paths(&a), ["..\\Images\\other.png", "..\\Images\\photo.png"]);
    // A file whose paths didn't change is copied as it is.
    let b = e.iter().find(|(n, _)| n == "Xrefs/b.dxf").unwrap();
    assert_eq!(b.1, std::fs::read(dir.join("parts/b.dxf")).unwrap());
    // The missing xref is reported, and the report lists the files.
    assert!(r["left"].as_array().unwrap().iter().any(|l| l["name"] == "gone.dxf" && l["note"] == "not found"), "{}", r["left"]);
    let report = String::from_utf8(e.iter().find(|(n, _)| n == "transmittal.txt").unwrap().1.clone()).unwrap();
    assert!(report.contains("Xrefs/b.dxf") && report.contains("gone.dxf"), "{report}");
}

#[test]
fn a_packed_folder_opens_with_its_xrefs_loaded() {
    let (dir, mut s) = project("folder");
    let out = dir.join("package");
    s.execute("etransmit", &json!({ "path": out.to_string_lossy() })).unwrap();
    // Move the package away from the originals: everything still resolves.
    let moved = folder("folder-moved").join("pkg");
    std::fs::rename(&out, &moved).unwrap();
    let mut t = Session::new();
    t.execute("open", &json!({ "path": moved.join("host.dxf").to_string_lossy() })).unwrap();
    let d = t.doc().unwrap();
    let a = d.block("a").unwrap();
    assert_eq!((a.xref_status, a.entities.len() >= 2), (XrefStatus::Loaded, true), "{:?}", a.xref_reason);
    assert!(d.block("a|b").is_some() || d.blocks.keys().any(|k| k.to_ascii_lowercase().contains("|b")), "the nested xref loads too");
    // A folder that isn't empty is refused.
    let e = s.execute("etransmit", &json!({ "path": moved.to_string_lossy() })).unwrap_err();
    assert!(e.to_string().contains("empty folder"), "{e}");
}

#[test]
fn flat_layout_listing_and_bad_options() {
    let (dir, mut s) = project("flat");
    let out = dir.join("flat.zip");
    s.execute("etransmit", &json!({ "path": out.to_string_lossy(), "layout": "flat", "report": false })).unwrap();
    let e = entries(&out);
    let mut names: Vec<&str> = e.iter().map(|(n, _)| n.as_str()).collect();
    names.sort();
    assert_eq!(names, ["a.dxf", "b.dxf", "host.dxf", "other.png", "photo.png"]);
    assert_eq!(drawing(&e, "host.dxf").block("a").unwrap().xref_path.as_deref(), Some("a.dxf"));
    assert_eq!(image_paths(&drawing(&e, "a.dxf")), ["other.png", "photo.png"]);
    // Without a path: only the list.
    let r = s.execute("etransmit", &json!({})).unwrap();
    assert_eq!(r["files"].as_array().unwrap().len(), 7);
    assert!(r.get("path").is_none());
    for p in [json!({ "layout": "tree" }), json!({ "path": "/dev/null/x.zip" })] {
        assert!(s.execute("etransmit", &p).is_err(), "{p}");
    }
    let _: Value = r;
}

#[test]
fn an_unsaved_drawing_is_packed_under_its_title() {
    hooks();
    let mut s = Session::new();
    s.execute("line", &json!({ "points": [[0, 0], [1, 1]] })).unwrap();
    let r = s.execute("etransmit", &json!({ "report": false })).unwrap();
    let files = r["files"].as_array().unwrap();
    assert_eq!(files.len(), 1);
    assert!(files[0]["target"].as_str().unwrap().ends_with(".dwg"), "{}", files[0]);
}

fn underlay(d: &mut Drawing, path: &str) {
    let u = serde_json::from_value(
        json!({ "insert": {"x":0,"y":0,"z":0}, "u": {"x":1,"y":0,"z":0}, "v": {"x":0,"y":1,"z":0}, "size": {"x":17,"y":11}, "path": path }),
    )
    .unwrap();
    let h = d.new_handle();
    d.model.push(Entity::new(h, EntityKind::Underlay(Box::new(u))));
}

/// Security suite (Q207 rework, packages): IMAGE, underlay and xref paths on another computer
/// are never read into a package; each is listed in `left` with the reason, and the package of
/// the rest is still made.
#[test]
fn paths_on_other_computers_are_left_out_of_the_package() {
    hooks();
    let dir = folder("network");
    let remote = [
        ("image", r"\\192.0.2.1\share\remote-photo.png"),
        ("image", "file://192.0.2.1/share/remote-scan.png"),
        ("underlay", r"\\192.0.2.1\share\remote-sheet.pdf"),
        ("underlay", "file://192.0.2.1/share/remote-specs.pdf"),
        ("xref", r"\\192.0.2.1\share\remote-base.dxf"),
        ("xref", "file://192.0.2.1/share/remote-site.dxf"),
    ];
    let mut host = Drawing::new_metric();
    line(&mut host, 0.0);
    for (i, (kind, path)) in remote.iter().enumerate() {
        match *kind {
            "image" => image(&mut host, path),
            "underlay" => underlay(&mut host, path),
            _ => xref(&mut host, &format!("net{i}"), path),
        }
    }
    // A local file beside them is still packed.
    image(&mut host, "img/photo.png");
    std::fs::create_dir_all(dir.join("img")).unwrap();
    std::fs::write(dir.join("img/photo.png"), b"photo bytes").unwrap();
    save(&host, &dir.join("host.dxf"));
    let mut s = Session::new();
    s.execute("open", &json!({ "path": dir.join("host.dxf").to_string_lossy() })).unwrap();
    let out = dir.join("out.zip");
    let r = s.execute("etransmit", &json!({ "path": out.to_string_lossy() })).unwrap();
    let mut left: Vec<(String, String, String)> = r["left"]
        .as_array()
        .unwrap()
        .iter()
        // The built-in font is listed in every package; the rest is what was left out.
        .filter(|l| l["kind"] != "font")
        .map(|l| (l["kind"].as_str().unwrap().to_string(), l["name"].as_str().unwrap().to_string(), l["note"].as_str().unwrap().to_string()))
        .collect();
    left.sort();
    let mut want: Vec<(String, String, String)> =
        remote.iter().map(|(k, p)| (k.to_string(), p.to_string(), "not packed: the path is on another computer".to_string())).collect();
    want.sort();
    assert_eq!(left, want, "{}", r["left"]);
    // Nothing was read from them: the package holds the host, the local image and the report.
    let mut names: Vec<String> = entries(&out).into_iter().map(|(n, _)| n).collect();
    names.sort();
    assert_eq!(names, ["Images/photo.png", "host.dxf", "transmittal.html", "transmittal.txt"]);
    let sources: Vec<&str> = r["files"].as_array().unwrap().iter().filter_map(|f| f["source"].as_str()).collect();
    assert!(sources.iter().all(|s| !s.contains("192.0.2.1")), "{sources:?}");
    std::fs::remove_dir_all(&dir).ok();
}

/// A font file holding only a `name` table with the given (name id, text) entries: enough for
/// the licence check.
fn font_bytes(names: &[(u16, &str)]) -> Vec<u8> {
    let strings: Vec<Vec<u8>> = names.iter().map(|(_, t)| t.encode_utf16().flat_map(u16::to_be_bytes).collect()).collect();
    let mut table = Vec::new();
    for v in [0u16, names.len() as u16, 6 + 12 * names.len() as u16] {
        table.extend(v.to_be_bytes());
    }
    let mut offset = 0u16;
    for ((id, _), s) in names.iter().zip(&strings) {
        for v in [3u16, 1, 0x409, *id, s.len() as u16, offset] {
            table.extend(v.to_be_bytes());
        }
        offset += s.len() as u16;
    }
    strings.iter().for_each(|s| table.extend(s));
    let mut out = Vec::new();
    out.extend(0x0001_0000u32.to_be_bytes());
    for v in [1u16, 16, 0, 0] {
        out.extend(v.to_be_bytes());
    }
    out.extend(*b"name");
    out.extend(0u32.to_be_bytes());
    out.extend(28u32.to_be_bytes());
    out.extend((table.len() as u32).to_be_bytes());
    out.extend(table);
    out
}

fn text_style(d: &mut Drawing, name: &str, font: &str) {
    d.text_styles.push(cadaddy_engine::doc::TextStyle { name: name.into(), font: font.into(), ..Default::default() });
}

fn open(path: &Path) -> Session {
    let mut s = Session::new();
    s.execute("open", &json!({ "path": path.to_string_lossy() })).unwrap();
    s
}

fn names(path: &Path) -> Vec<String> {
    let mut v: Vec<String> = entries(path).into_iter().map(|(n, _)| n).collect();
    v.sort();
    v
}

fn left_of(r: &Value) -> Vec<(String, String, String)> {
    let mut v: Vec<(String, String, String)> = r["left"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| (l["kind"].as_str().unwrap().into(), l["name"].as_str().unwrap().into(), l["note"].as_str().unwrap().into()))
        .collect();
    v.sort();
    v
}

/// Font licence rule: only a font whose own licence entries say SIL Open Font License goes in the
/// package (under Fonts/, with that note); a font of another licence, an SHX file and the
/// built-in stroke font are only listed in `left`, with their reasons, and are not in the ZIP.
#[test]
fn only_open_font_license_fonts_are_packed() {
    hooks();
    let dir = folder("fonts");
    let ofl = "This Font Software is licensed under the SIL Open Font License, Version 1.1.";
    cadaddy_fonts::ttf::register("etx-open-sans.ttf", font_bytes(&[(1, "Etx Open Sans"), (13, ofl)]));
    cadaddy_fonts::ttf::register("etx-closed-sans.ttf", font_bytes(&[(1, "Etx Closed Sans"), (13, "All rights reserved. Not for redistribution.")]));
    cadaddy_fonts::ttf::register(
        "etx-denied-sans.ttf",
        font_bytes(&[(1, "Etx Denied Sans"), (13, "This font is not licensed under the SIL Open Font License.")]),
    );
    let mut host = Drawing::new_metric();
    line(&mut host, 0.0);
    text_style(&mut host, "Open", "etx-open-sans.ttf");
    text_style(&mut host, "Closed", "etx-closed-sans.ttf");
    text_style(&mut host, "Denied", "etx-denied-sans.ttf");
    text_style(&mut host, "Site", "etx-site.shx");
    text_style(&mut host, "Own", cadaddy_fonts::BUILTIN_FONT);
    save(&host, &dir.join("host.dxf"));
    let mut s = open(&dir.join("host.dxf"));
    let out = dir.join("out.zip");
    let r = s.execute("etransmit", &json!({ "path": out.to_string_lossy() })).unwrap();

    assert_eq!(names(&out), ["Fonts/etx-open-sans.ttf", "host.dxf", "transmittal.html", "transmittal.txt"]);
    let packed = entries(&out).into_iter().find(|(n, _)| n == "Fonts/etx-open-sans.ttf").unwrap().1;
    assert_eq!(packed, font_bytes(&[(1, "Etx Open Sans"), (13, ofl)]));
    let font = r["files"].as_array().unwrap().iter().find(|f| f["kind"] == "font").unwrap();
    assert_eq!((font["target"].as_str(), font["note"].as_str()), (Some("Fonts/etx-open-sans.ttf"), Some("SIL Open Font License")));

    let not_copied = "not copied: its licence doesn't say it may be passed on (only SIL Open Font License fonts are)";
    let mut want = vec![
        ("font".to_string(), "etx-closed-sans.ttf".to_string(), not_copied.to_string()),
        ("font".to_string(), "etx-denied-sans.ttf".to_string(), not_copied.to_string()),
        ("font".to_string(), "etx-site.shx".to_string(), "SHX font: not copied (its licence isn't known)".to_string()),
        ("font".to_string(), cadaddy_fonts::BUILTIN_FONT.to_string(), "CADaddy's own stroke font: built into CADaddy".to_string()),
    ];
    want.sort();
    // The default "Standard" style's own font is listed too; the four above are the ones asked for.
    let left = left_of(&r);
    for w in &want {
        assert!(left.contains(w), "{w:?} not in {left:?}");
    }
    assert!(left.iter().all(|(_, n, _)| n != "etx-open-sans.ttf"), "{left:?}");
    std::fs::remove_dir_all(&dir).ok();
}

/// PDF underlays, table data-link files and plot style tables: exact package entries, the paths
/// rewritten in the packed host, and the missing plot style table listed.
#[test]
fn underlays_data_links_and_plot_style_tables_are_packed() {
    hooks();
    let dir = folder("kinds");
    std::fs::create_dir_all(dir.join("pdf")).unwrap();
    std::fs::create_dir_all(dir.join("data")).unwrap();
    std::fs::write(dir.join("pdf/site-plan.pdf"), b"%PDF-1.4 site plan").unwrap();
    std::fs::write(dir.join("data/parts.csv"), "Part,Qty\nbolt,4\n").unwrap();
    let mut s = Session::new();
    s.execute("line", &json!({ "points": [[0, 0], [1, 0]] })).unwrap();
    let t = s.execute("table", &json!({ "at": [0, 10], "cols": 2, "rows": 1 })).unwrap();
    let th = cadaddy_engine::doc::Handle::parse_hex(t["handle"].as_str().unwrap()).unwrap();
    {
        let d = s.doc_mut().unwrap();
        d.modify_entity(th, |e| {
            if let EntityKind::Table(t) = &mut e.kind {
                t.data_link = Some(cadaddy_engine::doc::DataLink { path: "data/parts.csv".into(), start_row: 0, ..Default::default() });
            }
        })
        .unwrap();
        underlay(d, "pdf/site-plan.pdf");
        d.plot_style_tables.push(cadaddy_engine::doc::PlotStyleTable {
            name: "site.stb".into(),
            description: "Site".into(),
            kind: cadaddy_engine::doc::PlotStyleKind::Named,
            styles: vec![cadaddy_engine::doc::PlotStyle { name: "Normal".into(), ..Default::default() }],
        });
        let mut a = cadaddy_engine::doc::Layout::new("Sheet A", 1);
        a.page.plot_style_table = "site.stb".into();
        let mut b = cadaddy_engine::doc::Layout::new("Sheet B", 2);
        b.page.plot_style_table = "missing-table.ctb".into();
        d.layouts.push(a);
        d.layouts.push(b);
    }
    s.execute("saveas", &json!({ "path": dir.join("host.dxf").to_string_lossy() })).unwrap();
    let mut s = open(&dir.join("host.dxf"));
    let out = dir.join("out.zip");
    let r = s.execute("etransmit", &json!({ "path": out.to_string_lossy() })).unwrap();

    assert_eq!(
        names(&out),
        ["DataLinks/parts.csv", "PlotStyles/site.stb.json", "Underlays/site-plan.pdf", "host.dxf", "transmittal.html", "transmittal.txt"]
    );
    let all = entries(&out);
    let bytes = |n: &str| all.iter().find(|(e, _)| e == n).unwrap().1.clone();
    assert_eq!(bytes("Underlays/site-plan.pdf"), b"%PDF-1.4 site plan");
    assert_eq!(bytes("DataLinks/parts.csv"), b"Part,Qty\nbolt,4\n");
    let packed = drawing(&all, "host.dxf");
    let underlay_paths: Vec<String> =
        packed.model.iter().filter_map(|e| if let EntityKind::Underlay(u) = &e.kind { Some(u.path.clone()) } else { None }).collect();
    assert_eq!(underlay_paths, [r"Underlays\site-plan.pdf"]);
    let link_paths: Vec<String> = packed
        .model
        .iter()
        .filter_map(|e| if let EntityKind::Table(t) = &e.kind { t.data_link.as_ref().map(|l| l.path.clone()) } else { None })
        .collect();
    assert_eq!(link_paths, [r"DataLinks\parts.csv"]);
    assert!(packed.plot_style_tables.iter().any(|t| t.name == "site.stb"));
    let kinds: Vec<(&str, &str)> =
        r["files"].as_array().unwrap().iter().map(|f| (f["kind"].as_str().unwrap(), f["target"].as_str().unwrap())).collect();
    for k in [("underlay", "Underlays/site-plan.pdf"), ("data link", "DataLinks/parts.csv"), ("plot style table", "PlotStyles/site.stb.json")] {
        assert!(kinds.contains(&k), "{k:?} not in {kinds:?}");
    }
    let left: Vec<_> = left_of(&r).into_iter().filter(|(k, _, _)| k == "plot style table").collect();
    assert_eq!(left, [("plot style table".to_string(), "missing-table.ctb".to_string(), "not found".to_string())]);
    std::fs::remove_dir_all(&dir).ok();
}

/// A file that can't be read is not in the package and no packed drawing has its path rewritten
/// to a package target, however many drawings refer to it: it is listed as "couldn't be read".
#[cfg(unix)]
#[test]
fn an_unreadable_file_leaves_no_dangling_path() {
    hooks();
    let dir = folder("unreadable");
    // Files that exist but can't be opened (permission denied).
    let mut sealed = Drawing::new_metric();
    line(&mut sealed, 5.0);
    save(&sealed, &dir.join("parts/broken.dxf"));
    std::fs::create_dir_all(dir.join("img")).unwrap();
    std::fs::write(dir.join("img/pic.png"), b"pic").unwrap();
    use std::os::unix::fs::PermissionsExt;
    for f in ["parts/broken.dxf", "img/pic.png"] {
        std::fs::set_permissions(dir.join(f), std::fs::Permissions::from_mode(0o000)).unwrap();
    }
    if std::fs::File::open(dir.join("img/pic.png")).is_ok() {
        // Running as a user the permission bits don't bind (root): nothing to prove here.
        std::fs::remove_dir_all(&dir).ok();
        return;
    }
    let mut nested = Drawing::new_metric();
    line(&mut nested, 1.0);
    xref(&mut nested, "broken", "broken.dxf");
    image(&mut nested, "../img/pic.png");
    save(&nested, &dir.join("parts/nested.dxf"));
    let mut host = Drawing::new_metric();
    xref(&mut host, "nested", "parts/nested.dxf");
    xref(&mut host, "broken", "parts/broken.dxf");
    image(&mut host, "img/pic.png");
    save(&host, &dir.join("host.dxf"));
    let mut s = open(&dir.join("host.dxf"));
    let out = dir.join("out.zip");
    let r = s.execute("etransmit", &json!({ "path": out.to_string_lossy() })).unwrap();

    assert_eq!(names(&out), ["Xrefs/nested.dxf", "host.dxf", "transmittal.html", "transmittal.txt"]);
    let all = entries(&out);
    let xref_paths = |d: &Drawing| {
        let mut v: Vec<String> = d.blocks.values().filter_map(|b| b.xref_path.clone()).collect();
        v.sort();
        v
    };
    // The host's xref to the unreadable drawing keeps its own path, as does the nested one's.
    assert_eq!(xref_paths(&drawing(&all, "host.dxf")), [r"Xrefs\nested.dxf", "parts/broken.dxf"]);
    assert_eq!(xref_paths(&drawing(&all, "Xrefs/nested.dxf")), ["broken.dxf"]);
    assert_eq!(image_paths(&drawing(&all, "host.dxf")), ["img/pic.png"]);
    assert_eq!(image_paths(&drawing(&all, "Xrefs/nested.dxf")), ["../img/pic.png"]);
    let left: Vec<_> = left_of(&r).into_iter().filter(|(k, _, _)| k != "font").collect();
    assert!(left.iter().all(|(_, _, n)| n.starts_with("couldn't be read")), "{left:?}");
    let mut named: Vec<&str> = left.iter().map(|(_, n, _)| n.as_str()).collect();
    named.sort();
    // Each unreadable file is listed once, under the path of its first reference.
    assert_eq!(named, ["../img/pic.png", "parts/broken.dxf"]);
    std::fs::remove_dir_all(&dir).ok();
}
