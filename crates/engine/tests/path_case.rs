//! File names ignore case, and `\` separates folders, as on Windows (Q117): xrefs saved with
//! Windows paths, typed file names, scripts, LISP `findfile`/`load`. The files are lower-case
//! and asked for in mixed case, so on a case-sensitive disk (Linux) these fail without the fix.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cadaddy_engine::Session;
use cadaddy_engine::cmd::file::{IoHooks, set_io};
use cadaddy_engine::doc::geom::Vec3;
use cadaddy_engine::doc::{Block, Drawing, Entity, EntityKind, Insert, Line, XrefStatus};
use serde_json::{Value, json};

fn hooks() {
    set_io(IoHooks {
        read: |b, name| cadaddy_io::read(b, name).map_err(|e| e.to_string()),
        write: |d, name| cadaddy_io::write(d, name).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
    });
}

fn folder(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cadaddy-pathcase-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn save(d: &Drawing, path: &Path) {
    std::fs::write(path, cadaddy_io::write(d, &path.to_string_lossy()).unwrap()).unwrap();
}

fn base() -> Drawing {
    let mut d = Drawing::new_metric();
    let h = d.new_handle();
    d.model.push(Entity::new(h, EntityKind::Line(Line { a: Vec3::ZERO, b: Vec3::new(10.0, 0.0, 0.0) })));
    d
}

/// A host drawing referencing `stored` (the path as a Windows drawing saves it).
fn host(stored: &str) -> Drawing {
    let mut d = Drawing::new_metric();
    let mut b = Block::new("Base");
    b.xref_path = Some(stored.into());
    d.blocks.insert("Base".into(), Arc::new(b));
    let h = d.new_handle();
    d.model.push(Entity::new(
        h,
        EntityKind::Insert(Insert {
            block: "Base".into(),
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
    d
}

#[test]
fn xrefs_saved_with_windows_paths_load_from_lower_case_files() {
    hooks();
    let dir = folder("xref");
    std::fs::create_dir_all(dir.join("xref")).unwrap();
    std::fs::create_dir_all(dir.join("sheets")).unwrap();
    save(&base(), &dir.join("xref/base.dxf"));
    for stored in [r"..\XREF\Base.DXF", "../Xref/BASE.dxf", r"..\xref\base.dxf"] {
        let path = dir.join("sheets/host.dxf");
        save(&host(stored), &path);
        let mut s = Session::new();
        s.execute("open", &json!({ "path": path.to_string_lossy() })).unwrap();
        let b = s.doc().unwrap().block("Base").map(|b| (b.xref_status, b.entities.len()));
        assert_eq!(b, Some((XrefStatus::Loaded, 1)), "{stored}");
    }
    // Typed by the user: XATTACH with a mixed-case relative path.
    let path = dir.join("sheets/host2.dxf");
    save(&Drawing::new_metric(), &path);
    let mut s = Session::new();
    s.execute("open", &json!({ "path": path.to_string_lossy() })).unwrap();
    let r = s.execute("xattach", &json!({ "path": r"..\XRef\Base.Dxf", "name": "B2" })).unwrap();
    assert_eq!(r["name"], "B2");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn opening_scripts_and_lisp_find_files_ignoring_case() {
    hooks();
    let dir = folder("files");
    std::fs::create_dir_all(dir.join("plans")).unwrap();
    save(&base(), &dir.join("plans/level1.dxf"));
    std::fs::write(dir.join("plans/draw.scr"), "LINE 0,0 5,5\n\n").unwrap();
    std::fs::write(dir.join("plans/tools.lsp"), "(defun pathcase-ok () 42)").unwrap();
    let mixed = |rel: &str| format!("{}{}{}", dir.to_string_lossy(), std::path::MAIN_SEPARATOR, rel);
    let mut s = Session::new();
    // Scripts and LISP load from trusted folders (SECURELOAD): this one and its subfolders.
    s.settings.trustedpaths = format!("{}{}...", dir.to_string_lossy(), std::path::MAIN_SEPARATOR);
    s.execute("open", &json!({ "path": mixed(r"PLANS\Level1.DXF") })).unwrap();
    assert_eq!(s.doc().unwrap().model.len(), 1);
    s.execute("script", &json!({ "path": mixed("Plans/DRAW.SCR") })).unwrap();
    assert_eq!(s.doc().unwrap().model.len(), 2, "the script ran");
    let lisp = |s: &mut Session, code: &str| -> Value { s.execute("lisp", &json!({ "code": code })).unwrap()["value"].clone() };
    let found = lisp(&mut s, &format!("(findfile \"{}\")", mixed("PLANS/Tools.LSP").replace('\\', "/")));
    assert!(found.as_str().is_some_and(|f| f.to_lowercase().ends_with("tools.lsp\"")), "{found}");
    let missing = lisp(&mut s, &format!("(findfile \"{}\")", mixed("plans/none.lsp").replace('\\', "/")));
    assert_eq!(missing, json!("nil"));
    lisp(&mut s, &format!("(load \"{}\")", mixed("Plans/TOOLS.lsp").replace('\\', "/")));
    assert_eq!(lisp(&mut s, "(pathcase-ok)"), json!("42"));
    let _ = std::fs::remove_dir_all(&dir);
}
