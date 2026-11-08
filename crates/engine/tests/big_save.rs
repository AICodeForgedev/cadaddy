//! Saving a large drawing as DXF (Q6279): a save too big to reopen is refused from a count before
//! anything is touched (the file, its .bak, a temporary file), without building the file in
//! memory; one that goes ahead is written into the file as it is made, the same bytes as the
//! file written whole. EXPORT to DXF too.
//!
//! A program of its own: it installs the file hooks (with the streamed writer) and counts every
//! allocation (a global allocator).
#![cfg(not(target_arch = "wasm32"))]

use std::sync::{Mutex, MutexGuard};

use cadaddy_engine::Session;
use cadaddy_engine::cmd::file::{IoHooks, SaveAs, set_io, set_write_as, set_write_streamed};
use cadaddy_engine::doc::geom::Vec3;
use cadaddy_engine::doc::{Common, Drawing, EntityKind, Line, Mesh, MeshFace, MeshKind, Space};
use serde_json::json;

#[global_allocator]
static ALLOC: cadaddy_memcount::Counting = cadaddy_memcount::Counting::new();

static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

fn hooks() {
    set_io(IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
        read_dwf: None,
        write_dwfx: None,
    });
    set_write_as(|d, n, o| cadaddy_io::write_with(d, n, &opts(o)).map_err(|e| e.to_string()));
    set_write_streamed(|d, n, o, open| cadaddy_io::write_streamed(d, n, &opts(o), open).map_err(|e| e.to_string()));
}

fn opts(o: &SaveAs) -> cadaddy_io::SaveOptions {
    cadaddy_io::SaveOptions { version: o.version, binary: o.binary }
}

/// A session with a drawing of `n` boxes (polyface meshes) and lines.
fn session(n: usize) -> Session {
    let mut d = Drawing::new_imperial();
    for i in 0..n {
        let (x, y) = ((i % 200) as f64 * 10.0, (i / 200) as f64 * 10.0);
        let kind = if i % 4 == 3 {
            EntityKind::Line(Line { a: Vec3::new(x, y, 0.0), b: Vec3::new(x + 6.0, y + 3.0, 0.0) })
        } else {
            let v = |dx: f64, dy: f64, z: f64| Vec3::new(x + dx, y + dy, z);
            let f = |a: [u32; 4]| MeshFace { v: a.to_vec(), hidden: 0 };
            EntityKind::Mesh(Mesh {
                vertices: vec![
                    v(0.0, 0.0, 0.0),
                    v(4.0, 0.0, 0.0),
                    v(4.0, 4.0, 0.0),
                    v(0.0, 4.0, 0.0),
                    v(0.0, 0.0, 3.0),
                    v(4.0, 0.0, 3.0),
                    v(4.0, 4.0, 3.0),
                    v(0.0, 4.0, 3.0),
                ],
                faces: vec![f([0, 1, 2, 3]), f([4, 5, 6, 7]), f([0, 1, 5, 4]), f([1, 2, 6, 5]), f([2, 3, 7, 6]), f([3, 0, 4, 7])],
                kind: MeshKind::Polyface,
            })
        };
        let _ = d.add(&Space::Model, Common::default(), kind);
    }
    let mut s = Session::new();
    s.open_drawing(d, "Sample Floor Plan", None);
    s
}

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("cadaddy-big-save-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch folder");
    dir
}

/// The names in `dir`, sorted.
fn names(dir: &std::path::Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    v.sort();
    v
}

#[test]
fn a_save_too_big_to_reopen_touches_nothing_and_a_forced_one_is_written_as_made() {
    let _g = serial();
    hooks();
    let dir = scratch("save");
    let path = dir.join("plan.dxf");
    let p = path.to_string_lossy().into_owned();
    std::fs::write(&path, b"the previous plan").expect("previous file");
    let mut s = session(20_000);
    // The limit lowered so twenty thousand objects are over it (about two million groups).
    let before = ALLOC.live();
    ALLOC.reset_peak();
    let refused = cadaddy_dxf::with_max_tags(500_000, || s.execute("saveas", &json!({ "path": p })));
    let held = ALLOC.peak().saturating_sub(before);
    let e = refused.expect_err("over the limit").to_string();
    assert!(e.contains("too large to save as DXF") && e.contains(".cdy") && e.contains("force:true"), "{e}");
    assert_eq!(std::fs::read(&path).expect("file"), b"the previous plan", "the refused save changed the file");
    assert_eq!(names(&dir), ["plan.dxf"], "a refused save left a .bak or a temporary file");
    // Its groups alone would be about 80 MB; the count holds a few.
    assert!(held < 16 << 20, "refusing held {} MB", held >> 20);

    // Forced: written into the file, the same bytes as written whole, the previous one kept.
    let r = cadaddy_dxf::with_max_tags(500_000, || s.execute("saveas", &json!({ "path": p, "force": true }))).expect("forced save");
    let whole = cadaddy_dxf::with_oversize_allowed(true, || cadaddy_io::write(s.doc().expect("doc"), "plan.dxf")).expect("whole");
    let written = std::fs::read(&path).expect("saved");
    assert!(written == whole, "the streamed file differs from the whole one ({} / {} bytes)", written.len(), whole.len());
    assert_eq!(r["bytes"], json!(whole.len()));
    assert_eq!(names(&dir), ["plan.dxf", "plan.dxf.bak"]);
    assert_eq!(std::fs::read(dir.join("plan.dxf.bak")).expect("bak"), b"the previous plan");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn export_to_dxf_is_written_as_made_and_refused_before_it() {
    let _g = serial();
    hooks();
    let dir = scratch("export");
    let path = dir.join("out.dxf");
    let p = path.to_string_lossy().into_owned();
    let mut s = session(2_000);
    let refused = cadaddy_dxf::with_max_tags(10_000, || s.execute("export", &json!({ "path": p })));
    assert!(refused.expect_err("over the limit").to_string().contains("too large to save as DXF"));
    assert!(names(&dir).is_empty(), "a refused export left a file: {:?}", names(&dir));
    let r = s.execute("export", &json!({ "path": p })).expect("export");
    let whole = cadaddy_io::write(s.doc().expect("doc"), "export.dxf").expect("whole");
    assert!(std::fs::read(&path).expect("exported") == whole);
    assert_eq!(r["bytes"], json!(whole.len()));
    let _ = std::fs::remove_dir_all(&dir);
}
