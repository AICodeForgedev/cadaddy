//! Opening and saving never quietly loses work: a file that is already open is switched to, not
//! opened a second time; saving over a file that was read with parts left out asks first; saving
//! over a drawing keeps the previous version as .bak; a failed save leaves no temporary file.

use cadaddy_doc::Drawing;
use cadaddy_engine::Session;
use cadaddy_engine::cmd::file::{IoHooks, set_io};
use serde_json::json;

/// A reader that leaves something out when the file says so, and a writer that writes "saved".
/// The IO hooks, and a lock held for the test: opening a drawing sets the process-wide folder
/// relative image paths are taken from, so these tests take turns.
fn install() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    set_io(IoHooks {
        read: |bytes, _| {
            let mut d = Drawing::new_metric();
            if bytes.starts_with(b"LOSSY") {
                d.read_notes.push("DXF: the file ends early".into());
            }
            if bytes.starts_with(b"BADSTYLE") {
                d.header.set_str("DIMSTYLE", "NODIM");
            }
            Ok(d)
        },
        write: |_, _| Ok(b"saved".to_vec()),
        plot: None,
        read_pdf: None,
    });
    guard
}

fn dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-open-save-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn opening_an_open_file_switches_to_it() {
    let _turn = install();
    let d = dir("twice");
    let path = d.join("plan.dxf");
    std::fs::write(&path, b"plan").unwrap();
    let p = path.to_string_lossy().to_string();
    let mut s = Session::new();
    let first = s.execute("open", &json!({ "path": p })).unwrap()["index"].as_u64().unwrap() as usize;
    s.cmdline("circle 0,0 5").unwrap();
    s.new_drawing(false);
    let tabs = s.docs.len();
    // Opened again (also through a different spelling of the same path): no second copy, and
    // the drawing with the unsaved circle becomes current.
    let again = d.join(".").join("plan.dxf").to_string_lossy().to_string();
    let r = s.execute("open", &json!({ "path": again })).unwrap();
    assert_eq!(r["alreadyOpen"], json!(true));
    assert_eq!(s.docs.len(), tabs);
    assert_eq!(s.active, first);
    assert_eq!(s.doc().unwrap().entity_count(), 1);
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn saving_over_a_file_read_with_parts_missing_asks_first_and_keeps_a_bak() {
    let _turn = install();
    let d = dir("lossy");
    let path = d.join("mech.dxf");
    std::fs::write(&path, b"LOSSY original").unwrap();
    let p = path.to_string_lossy().to_string();
    let mut s = Session::new();
    s.execute("open", &json!({ "path": p })).unwrap();
    s.cmdline("circle 0,0 5").unwrap();
    // The first save over the original explains and stops; the file is untouched.
    let e = s.execute("qsave", &json!({})).unwrap_err().to_string();
    assert!(e.contains("parts missing") && e.contains("mech.bak"), "{e}");
    assert_eq!(std::fs::read(&path).unwrap(), b"LOSSY original");
    // Saving again overwrites, keeping the original as .bak.
    s.execute("qsave", &json!({})).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"saved");
    assert_eq!(std::fs::read(d.join("mech.bak")).unwrap(), b"LOSSY original");
    // From then on the file holds all of the drawing: saves don't ask.
    s.execute("qsave", &json!({})).unwrap();
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn saveas_elsewhere_doesnt_ask_and_a_complete_file_saves_at_once() {
    let _turn = install();
    let d = dir("elsewhere");
    let lossy = d.join("a.dxf");
    std::fs::write(&lossy, b"LOSSY").unwrap();
    let mut s = Session::new();
    s.execute("open", &json!({ "path": lossy.to_string_lossy() })).unwrap();
    // Another name keeps the original as it was: no question.
    s.execute("saveas", &json!({ "path": d.join("b.dxf").to_string_lossy() })).unwrap();
    assert_eq!(std::fs::read(&lossy).unwrap(), b"LOSSY");
    // ...and the new file is complete, so saving it again doesn't ask either.
    s.execute("qsave", &json!({})).unwrap();
    // A file read whole saves over itself straight away (with its .bak).
    let whole = d.join("c.dxf");
    std::fs::write(&whole, b"whole").unwrap();
    s.execute("open", &json!({ "path": whole.to_string_lossy() })).unwrap();
    s.execute("qsave", &json!({})).unwrap();
    assert_eq!(std::fs::read(d.join("c.bak")).unwrap(), b"whole");
    std::fs::remove_dir_all(&d).ok();
}

#[cfg(unix)]
#[test]
fn a_failed_save_leaves_no_temporary_file() {
    use std::os::unix::fs::PermissionsExt;
    let _turn = install();
    let d = dir("readonly");
    let mut s = Session::new();
    s.cmdline("circle 0,0 5").unwrap();
    std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o555)).unwrap();
    let target = d.join("x.dxf");
    let failed = s.execute("saveas", &json!({ "path": target.to_string_lossy() })).is_err();
    std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o755)).unwrap();
    // (Running as root can write anyway; then there is nothing to check.)
    if failed {
        assert!(std::fs::read_dir(&d).unwrap().next().is_none(), "left behind: {:?}", std::fs::read_dir(&d).unwrap().collect::<Vec<_>>());
    }
    std::fs::remove_dir_all(&d).ok();
}

#[cfg(unix)]
#[test]
fn a_read_only_file_is_not_saved_over_and_permissions_are_kept() {
    use std::os::unix::fs::PermissionsExt;
    let _turn = install();
    let d = dir("perms");
    // A read-only drawing: QSAVE refuses (save it under another name), the file is untouched.
    let ro = d.join("ro.dxf");
    std::fs::write(&ro, b"original").unwrap();
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o444)).unwrap();
    let mut s = Session::new();
    s.execute("open", &json!({ "path": ro.to_string_lossy() })).unwrap();
    s.cmdline("circle 0,0 5").unwrap();
    let e = s.execute("qsave", &json!({})).unwrap_err().to_string();
    assert!(e.contains("read-only"), "{e}");
    assert_eq!(std::fs::read(&ro).unwrap(), b"original");
    // A drawing with its own permissions keeps them, and saving twice replaces the .bak.
    let rw = d.join("rw.dxf");
    std::fs::write(&rw, b"first").unwrap();
    std::fs::set_permissions(&rw, std::fs::Permissions::from_mode(0o750)).unwrap();
    s.execute("open", &json!({ "path": rw.to_string_lossy() })).unwrap();
    s.execute("qsave", &json!({})).unwrap();
    s.cmdline("circle 0,0 6").unwrap();
    s.execute("qsave", &json!({})).unwrap();
    assert_eq!(std::fs::metadata(&rw).unwrap().permissions().mode() & 0o777, 0o750);
    assert_eq!(std::fs::read(d.join("rw.bak")).unwrap(), b"saved");
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o644)).unwrap();
    std::fs::remove_dir_all(&d).ok();
}

/// Saving a drawing to the file another tab has open is refused: it would wipe that drawing's
/// work and leave two tabs on one file (battle test 3, B1).
#[test]
fn saving_over_a_drawing_open_in_another_tab_is_refused() {
    let _turn = install();
    let d = dir("sametab");
    let p = d.join("same.dxf").to_string_lossy().to_string();
    let mut s = Session::new();
    s.cmdline("circle 0,0 5").unwrap();
    s.execute("saveas", &json!({ "path": p })).unwrap();
    s.new_drawing(false);
    s.cmdline("line 0,0 9,9 ").unwrap();
    let e = s.execute("saveas", &json!({ "path": p })).unwrap_err().to_string();
    assert!(e.contains("open in another tab"), "{e}");
    assert_eq!(s.docs.iter().filter(|st| st.path.as_deref() == Some(p.as_str())).count(), 1);
    // The tab's own file saves as before.
    s.execute("document.switch", &json!({"index": 0})).unwrap();
    s.execute("qsave", &json!({})).unwrap();
    std::fs::remove_dir_all(&d).ok();
}

/// A drawing's relative image paths are found in its own folder whichever way it is opened (the
/// OPEN command here), never in the program's working folder; switching back to an open drawing
/// finds its images again.
#[test]
fn relative_images_are_found_beside_the_drawing_opened() {
    use cadaddy_engine::render::images;
    let _turn = install();
    let png = |rgba: [u8; 4]| images::Raster::new(1, 1, rgba.to_vec()).unwrap().encode_png().unwrap();
    let name = format!("pics/logo-{}.png", std::process::id());
    let (a, b) = (dir("img-a"), dir("img-b"));
    for (d, px) in [(&a, [255, 0, 0, 255]), (&b, [0, 0, 255, 255])] {
        std::fs::create_dir_all(d.join("pics")).unwrap();
        std::fs::write(d.join(&name), png(px)).unwrap();
        std::fs::write(d.join("plan.dxf"), b"plan").unwrap();
    }
    let mut s = Session::new();
    let open = |s: &mut Session, d: &std::path::Path| s.execute("open", &json!({ "path": d.join("plan.dxf").to_string_lossy() })).unwrap();
    let ia = open(&mut s, &a)["index"].as_u64().unwrap() as usize;
    assert_eq!(images::get(&name).and_then(|r| r.pixel(0, 0)), Some([255, 0, 0, 255]));
    open(&mut s, &b);
    assert_eq!(images::get(&name).and_then(|r| r.pixel(0, 0)), Some([0, 0, 255, 255]));
    // Back to the first drawing (already open).
    open(&mut s, &a);
    assert_eq!(images::get(&name).and_then(|r| r.pixel(0, 0)), Some([255, 0, 0, 255]));
    // A drawing whose folder lacks the image doesn't borrow another drawing's of the same name.
    let c = dir("img-c");
    std::fs::write(c.join("plan.dxf"), b"plan").unwrap();
    open(&mut s, &c);
    assert!(images::get(&name).is_none(), "an image from another drawing's folder");
    // Making another open drawing current (a tab click) takes its folder again.
    s.active = ia;
    cadaddy_engine::cmd::file::sync_image_folder(&s);
    assert_eq!(images::get(&name).and_then(|r| r.pixel(0, 0)), Some([255, 0, 0, 255]));
    for d in [&a, &b, &c] {
        std::fs::remove_dir_all(d).ok();
    }
}

#[test]
fn the_open_reply_lists_reset_current_styles() {
    let _turn = install();
    let d = dir("badstyle");
    let path = d.join("styles.dxf");
    std::fs::write(&path, b"BADSTYLE").unwrap();
    let mut s = Session::new();
    let r = s.execute("open", &json!({ "path": path.to_string_lossy() })).unwrap();
    let warnings = r["warnings"].as_array().unwrap();
    assert!(warnings.iter().any(|w| w.as_str().is_some_and(|w| w.contains("NODIM") && w.contains("Standard"))), "{r}");
    assert_eq!(s.doc().unwrap().header.str("DIMSTYLE", ""), "Standard");
    std::fs::remove_dir_all(&d).ok();
}
