//! Incremental .cdy saves (Q99): QSAVE appends only what changed to the file the drawing was saved
//! to or read from, a file changed by anything else is written in full, a save killed partway
//! leaves the previous save readable, and autosave writes .cdy without blocking.

use std::path::PathBuf;
use std::sync::Arc;

use cadaddy_engine::Session;
use cadaddy_engine::cmd::file::{CdyHooks, CdyState, CdyWrite, IoHooks, set_cdy_hooks, set_io};
use serde_json::json;

fn hooks() {
    set_io(IoHooks {
        read: |b, name| cadaddy_io::read(b, name).map_err(|e| e.to_string()),
        write: |d, name| cadaddy_io::write(d, name).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
        read_dwf: None,
        write_dwfx: None,
    });
    // As the apps install it.
    set_cdy_hooks(CdyHooks {
        save: |d, prev| {
            let prev = prev.and_then(|p| p.as_ref().downcast_ref::<cadaddy_io::cdy::SaveState>());
            let (out, state) = cadaddy_io::cdy::save(d, prev, &|| None)?;
            let out = match out {
                cadaddy_io::cdy::Output::Full(bytes) => CdyWrite::Full(bytes),
                cadaddy_io::cdy::Output::Append { from, bytes, index_at } => CdyWrite::Append { from, bytes, index_at },
            };
            Ok((out, Arc::new(state)))
        },
        seed: |d, bytes| cadaddy_io::cdy::seed(d, bytes).map(|s| Arc::new(s) as CdyState),
    });
}

fn folder(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cadaddy-cdyfast-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A session with `n` lines drawn.
fn drawn(n: usize) -> Session {
    hooks();
    let mut s = Session::new();
    let pts: Vec<_> = (0..n).map(|i| json!([[i, 0], [i, 1]])).collect();
    for p in pts {
        s.execute("line", &json!({ "points": p })).unwrap();
    }
    s
}

fn len(p: &str) -> u64 {
    std::fs::metadata(p).unwrap().len()
}

#[test]
fn qsave_appends_only_what_changed() {
    let dir = folder("qsave");
    let p = dir.join("Big.cdy").to_string_lossy().into_owned();
    let mut s = drawn(3000);
    let r = s.execute("saveas", &json!({ "path": p })).unwrap();
    assert!(r.get("incremental").is_none(), "the first save is a full one: {r}");
    let first = len(&p);
    s.execute("line", &json!({ "points": [[0, 5], [9, 5]] })).unwrap();
    let r = s.execute("qsave", &json!({})).unwrap();
    assert_eq!(r["incremental"], json!(true), "{r}");
    assert!(r["bytes"].as_u64().unwrap() < first / 2, "{r} (file was {first})");
    assert_eq!(len(&p), first + r["bytes"].as_u64().unwrap(), "appended, not rewritten");
    // Read back exactly; and the drawing opened from it saves incrementally too.
    let mut t = Session::new();
    t.execute("open", &json!({ "path": p })).unwrap();
    assert_eq!(t.doc().unwrap().model, s.doc().unwrap().model);
    t.execute("erase", &json!({ "handles": [t.doc().unwrap().model.iter().next().unwrap().handle.hex()] })).unwrap();
    let r = t.execute("qsave", &json!({})).unwrap();
    assert_eq!(r["incremental"], json!(true), "{r}");
    let mut u = Session::new();
    u.execute("open", &json!({ "path": p })).unwrap();
    assert_eq!(u.doc().unwrap().model, t.doc().unwrap().model);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_file_changed_by_something_else_is_written_in_full() {
    let dir = folder("changed");
    let p = dir.join("Shared.cdy").to_string_lossy().into_owned();
    let mut s = drawn(50);
    s.execute("saveas", &json!({ "path": p })).unwrap();
    // Another program (or copy) replaces the file.
    let mut other = drawn(5);
    other.execute("saveas", &json!({ "path": dir.join("other.cdy").to_string_lossy() })).unwrap();
    std::fs::copy(dir.join("other.cdy"), &p).unwrap();
    s.execute("line", &json!({ "points": [[0, 5], [9, 5]] })).unwrap();
    // The save is refused; forced, it never appends to a file it didn't write: it writes in full.
    assert!(s.execute("qsave", &json!({})).unwrap_err().to_string().contains("changed on disk"));
    let r = s.execute("qsave", &json!({ "force": true })).unwrap();
    assert!(r.get("incremental").is_none(), "{r}");
    let mut t = Session::new();
    t.execute("open", &json!({ "path": p })).unwrap();
    assert_eq!(t.doc().unwrap().model, s.doc().unwrap().model);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Run by [`a_writer_killed_mid_save_leaves_a_readable_file`] in a child process: keep editing
/// every chunk of a large drawing and saving it, until killed.
#[test]
fn child_keeps_saving() {
    let Ok(p) = std::env::var("CADADDY_CDY_CHILD") else { return };
    let mut s = Session::new();
    hooks();
    s.execute("open", &json!({ "path": p })).unwrap();
    let handles: Vec<String> = s.doc().unwrap().model.iter().step_by(512).map(|e| e.handle.hex()).collect();
    for i in 0..1_000_000u64 {
        for h in &handles {
            s.execute("move", &json!({ "handles": [h], "from": [0, 0], "to": [0, (i % 7) as f64] })).unwrap();
        }
        s.execute("qsave", &json!({})).unwrap();
        std::fs::write(format!("{p}.saves"), i.to_string()).unwrap();
    }
}

#[test]
fn a_writer_killed_mid_save_leaves_a_readable_file() {
    let dir = folder("kill");
    let p = dir.join("Killed.cdy").to_string_lossy().into_owned();
    let mut s = drawn(30_000);
    s.execute("saveas", &json!({ "path": p })).unwrap();
    let n = s.doc().unwrap().model.len();
    let exe = std::env::current_exe().unwrap();
    for (round, ms) in [150u64, 400, 700, 1100, 1700].into_iter().enumerate() {
        let mut child = std::process::Command::new(&exe)
            .args(["child_keeps_saving", "--exact", "--nocapture", "--test-threads", "1"])
            .env("CADADDY_CDY_CHILD", &p)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(ms));
        child.kill().unwrap();
        let _ = child.wait();
        // Whatever the moment, the file opens whole: no part left out, every object there.
        let bytes = std::fs::read(&p).unwrap();
        let (d, rep) = cadaddy_io::cdy::read_cdy_report(&bytes).unwrap_or_else(|e| panic!("round {round}: {e}"));
        assert!(rep.notes.iter().all(|n| n.contains("index")), "round {round}: {:?}", rep.notes);
        assert_eq!(d.model.len(), n, "round {round}");
    }
    let saves = std::fs::read_to_string(format!("{p}.saves")).unwrap_or_default();
    assert!(!saves.is_empty(), "the child saved at least once");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn autosave_writes_cdy_without_blocking() {
    use cadaddy_engine::autosave::{Autosaver, Job};
    let dir = folder("autosave");
    let s = drawn(1);
    let mut big = s.doc().unwrap().clone();
    for i in 0..150_000u64 {
        let x = i as f64;
        let _ = big.add(
            &cadaddy_engine::doc::Space::Model,
            Default::default(),
            cadaddy_engine::doc::EntityKind::Point(cadaddy_engine::doc::Point { p: cadaddy_engine::doc::geom::Vec3::new(x, x, 0.0), angle: 0.0 }),
        );
    }
    let path = dir.join("Big_1-a_1.sv$").to_string_lossy().into_owned();
    let mut saver = Autosaver::new("1-a");
    let t = std::time::Instant::now();
    saver.perform(vec![Job::Write { uid: 1, path: path.clone(), doc: Arc::new(big.clone()) }]);
    let returned = t.elapsed();
    let mut waited = 0;
    while !std::path::Path::new(&path).exists() && waited < 600 {
        std::thread::sleep(std::time::Duration::from_millis(50));
        saver.reap();
        waited += 1;
    }
    let written = t.elapsed();
    assert!(returned * 4 < written, "perform returned in {returned:?}, the write took {written:?}");
    let back = cadaddy_io::read(&std::fs::read(&path).unwrap(), &path).unwrap();
    assert!(cadaddy_io::cdy::is_cdy(&std::fs::read(&path).unwrap()));
    assert_eq!(back.model, big.model);
    let _ = std::fs::remove_dir_all(&dir);
}

// ─── A print set's sheets on demand (Q6418) ──────────────────────────────────────────────────

/// A made-up print set of `sheets` sheets of about `per` objects saved to `<dir>/set.cdy` by a
/// headless session (as the job leaves one), with its object count.
fn saved_set(dir: &std::path::Path, sheets: usize, per: usize) -> (String, usize) {
    hooks();
    let mut s = Session::new();
    s.open_drawing(cadaddy_engine::sample::print_set(sheets, per), "Sample Set", None);
    let n = s.doc().unwrap().model.len();
    let p = dir.join("set.cdy").to_string_lossy().into_owned();
    s.execute("saveas", &json!({ "path": p })).unwrap();
    (p, n)
}

/// `path` opened as the interface opens it: sheets on demand, read on a worker thread, then on
/// its first sheet.
fn opened_on_demand(path: &str) -> Session {
    hooks();
    let mut s = Session::new();
    s.viewport_px = (1200.0, 800.0);
    s.sheets_on_demand = true;
    let f = cadaddy_engine::cmd::file::read_file_on_demand(path).unwrap();
    s.finish_open(f).unwrap();
    assert!(s.open_first_sheet());
    s
}

fn held(s: &Session) -> usize {
    s.doc().unwrap().model.len()
}

fn readings(s: &mut Session) -> Vec<serde_json::Value> {
    let mut run = |c: &str, p: serde_json::Value| s.execute(c, &p).unwrap_or_else(|e| panic!("{c}: {e}"));
    vec![
        run("entities", json!({"limit": 1}))["count"].clone(),
        run("takeoff", json!({})),
        run("takeoff", json!({"mode": "area"})),
        run("bim.tree", json!({}))["total"].clone(),
        run("count", json!({"block": "SYM-OUTLET", "space": "model"})),
    ]
}

#[test]
fn a_print_set_opens_in_the_interface_with_its_first_sheet_and_the_rest_out_of_memory() {
    let dir = folder("ondemand-open");
    let (p, total) = saved_set(&dir, 6, 3000);
    let s = opened_on_demand(&p);
    let d = s.doc().unwrap();
    assert!(d.sheet_store.has_parked());
    assert_eq!(held(&s) + d.sheet_store.parked_objects(), total, "every object is held or parked, once");
    // The first sheet and the chunks that straddle two sheets: well under two sheets of six.
    assert!(held(&s) < total / 3, "{} of {total} held", held(&s));
    // The open sheet is all there.
    let first = s.print_sheets()[0].rect;
    let whole = cadaddy_io::read(&std::fs::read(&p).unwrap(), &p).unwrap();
    let on_first = whole.model.iter().filter(|e| cadaddy_engine::doc::entity_bounds(&whole, e, 0).intersects(&first)).count();
    assert_eq!(s.view_doc().unwrap().model.len(), on_first);
    // Opening it is no change to save.
    assert!(!s.state().unwrap().is_dirty());
    // A headless session reads it whole, and never parks a sheet.
    let mut h = Session::new();
    h.execute("open", &json!({ "path": p })).unwrap();
    assert_eq!(held(&h), total);
    h.open_first_sheet();
    h.execute("sheet.next", &json!({})).unwrap();
    assert_eq!(held(&h), total);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn counts_takeoff_and_queries_are_the_same_with_the_sheets_out_of_memory() {
    let dir = folder("ondemand-readings");
    let (p, total) = saved_set(&dir, 5, 400);
    let mut whole = Session::new();
    whole.execute("open", &json!({ "path": p })).unwrap();
    let want = readings(&mut whole);
    let mut s = opened_on_demand(&p);
    assert!(s.has_parked_sheets());
    // Moving between sheets loads nothing else.
    assert!(!s.needs_whole_set("sheet.next") && !s.needs_whole_set("zoom") && !s.needs_whole_set("line"));
    assert!(s.needs_whole_set("entities") && s.needs_whole_set("takeoff") && s.needs_whole_set("job.start"));
    assert_eq!(readings(&mut s), want, "the same counts, takeoff and model tree");
    assert_eq!(held(&s), total, "a command reading the whole set had it loaded first");
    let order = |s: &Session| s.doc().unwrap().model.iter().map(|e| e.handle).collect::<Vec<_>>();
    assert_eq!(order(&s), order(&whole), "in the same draw order");
    assert!(!s.has_parked_sheets());
    // Loading is no change to save (what the commands themselves record is the same both ways).
    assert_eq!(s.state().unwrap().is_dirty(), whole.state().unwrap().is_dirty());
    let mut t = opened_on_demand(&p);
    t.load_all_sheets().unwrap();
    assert!(!t.has_parked_sheets() && !t.state().unwrap().is_dirty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn stepping_through_the_sheets_keeps_the_open_one_and_a_few_recent_in_memory() {
    let dir = folder("ondemand-step");
    let (p, total) = saved_set(&dir, 8, 2000);
    let mut s = opened_on_demand(&p);
    let mut most = 0;
    for i in 1..8 {
        s.execute("sheet.next", &json!({})).unwrap();
        assert_eq!(s.active_sheet().map(|(k, _)| k), Some(i));
        // The open sheet is in memory: its view holds what the whole set has on it.
        let rect = s.print_sheets()[i].rect;
        let d = s.doc().unwrap();
        assert!(d.sheet_store.parked_sheet(&s.print_sheets()[i].number, &rect).is_none(), "sheet {} is in memory", i + 1);
        assert_eq!(held(&s) + d.sheet_store.parked_objects(), total);
        most = most.max(held(&s));
    }
    // The open sheet and the recent ones (and the straddling chunks), never the set.
    let per = total / 8;
    assert!(most <= (cadaddy_engine::sheetstore::CACHE + 2) * per, "{most} held at most, {per} a sheet");
    // Every sheet loaded again: the draw order is the set's.
    let mut all = opened_on_demand(&p);
    for _ in 0..7 {
        all.execute("sheet.next", &json!({})).unwrap();
    }
    all.load_all_sheets().unwrap();
    let whole = cadaddy_io::read(&std::fs::read(&p).unwrap(), &p).unwrap();
    let order: Vec<_> = all.doc().unwrap().model.iter().map(|e| e.handle).collect();
    assert_eq!(order, whole.model.iter().map(|e| e.handle).collect::<Vec<_>>(), "loaded back in the set's draw order");
    // Going back to a sheet out of memory loads it again.
    s.execute("sheet.open", &json!({ "sheet": 1 })).unwrap();
    assert!(s.doc().unwrap().sheet_store.parked_sheet(&s.print_sheets()[0].number, &s.print_sheets()[0].rect).is_none());
    assert!(!s.state().unwrap().is_dirty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_edit_on_the_open_sheet_saves_and_the_other_sheets_are_kept_byte_for_byte() {
    let dir = folder("ondemand-save");
    let (p, total) = saved_set(&dir, 5, 2500);
    let before = std::fs::read(&p).unwrap();
    let mut s = opened_on_demand(&p);
    let parked: Vec<Vec<u8>> = s.doc().unwrap().sheet_store.parked.iter().flat_map(|x| x.chunks.iter()).map(|c| c.stored.clone()).collect();
    assert!(!parked.is_empty());
    let c = s.print_sheets()[0].rect.center();
    s.execute("line", &json!({ "points": [[c.x, c.y], [c.x + 40.0, c.y]] })).unwrap();
    assert!(s.has_parked_sheets(), "drawing a line loads nothing");
    let r = s.execute("qsave", &json!({})).unwrap();
    let after = std::fs::read(&p).unwrap();
    assert!(r["incremental"] == json!(true) || after.len() < before.len() * 2, "{r}");
    assert!(after.len() < before.len() + before.len() / 4, "{} bytes after a one-line edit, {} before", after.len(), before.len());
    for k in &parked {
        assert!(after.windows(k.len()).any(|w| w == k.as_slice()), "a sheet out of memory is saved as it was stored");
    }
    let back = cadaddy_io::read(&after, &p).unwrap();
    assert!(back.read_notes.is_empty(), "{:?}", back.read_notes);
    assert_eq!(back.model.len(), total + 1, "every sheet and the new line");
    // Saved as DWG/DXF it is the whole set, from a copy: the drawing keeps its sheets out.
    let dxf = dir.join("set.dxf").to_string_lossy().into_owned();
    s.execute("saveas", &json!({ "path": dxf })).unwrap();
    let back = cadaddy_io::read(&std::fs::read(&dxf).unwrap(), &dxf).unwrap();
    assert_eq!(back.model.len(), total + 1);
    assert!(s.has_parked_sheets(), "the export didn't load the set into the drawing");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn undo_and_reopening_after_sheets_moved_in_and_out_are_consistent() {
    let dir = folder("ondemand-undo");
    let (p, total) = saved_set(&dir, 6, 1500);
    let mut s = opened_on_demand(&p);
    let c = s.print_sheets()[0].rect.center();
    s.execute("line", &json!({ "points": [[c.x, c.y], [c.x + 40.0, c.y]] })).unwrap();
    for _ in 0..5 {
        s.execute("sheet.next", &json!({})).unwrap();
    }
    // Sheet 1 (with the line) is out of memory now; undo takes the line away all the same.
    s.execute("undo", &json!({})).unwrap();
    s.load_all_sheets().unwrap();
    assert_eq!(held(&s), total, "the line is undone, every object is there once");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_print_set_saved_before_sheet_sections_opens_whole_and_gets_them_on_its_next_save() {
    let dir = folder("ondemand-old");
    hooks();
    // As an earlier CADaddy wrote it: no sheet index.
    let set = cadaddy_engine::sample::print_set(4, 1500);
    let total = set.model.len();
    let p = dir.join("old.cdy").to_string_lossy().into_owned();
    std::fs::write(&p, cadaddy_io::cdy::write_cdy_checkpoint(&set).unwrap()).unwrap();
    let bytes = std::fs::read(&p).unwrap();
    assert!(cadaddy_io::cdy::verify(&bytes).unwrap()["sizes"].get("SHTI").is_none());
    let mut s = opened_on_demand(&p);
    // Read whole, then the sheets it isn't showing go out of memory.
    assert!(s.has_parked_sheets());
    assert_eq!(held(&s) + s.doc().unwrap().sheet_store.parked_objects(), total);
    s.execute("qsave", &json!({})).unwrap();
    let bytes = std::fs::read(&p).unwrap();
    assert!(cadaddy_io::cdy::verify(&bytes).unwrap()["sizes"].get("SHTI").is_some(), "migrated on save");
    let o = opened_on_demand(&p);
    assert!(o.has_parked_sheets());
    assert_eq!(cadaddy_io::read(&bytes, &p).unwrap().model.len(), total);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_building_models_reads_need_no_sheet_and_answer_the_same() {
    let dir = folder("ondemand-model");
    hooks();
    // A print set with a few model elements (they carry their build's extended data) on its sheets.
    let mut set = cadaddy_engine::sample::print_set(4, 1200);
    for i in 0..30 {
        let x = 100.0 + f64::from(i) * 40.0;
        let h = set
            .add(
                &cadaddy_engine::doc::Space::Model,
                Default::default(),
                cadaddy_engine::doc::EntityKind::Line(cadaddy_engine::doc::Line {
                    a: cadaddy_engine::doc::geom::Vec3::new(x, 100.0, 0.0),
                    b: cadaddy_engine::doc::geom::Vec3::new(x + 30.0, 100.0, 0.0),
                }),
            )
            .unwrap();
        let app = cadaddy_engine::cmd::wallgen::APP;
        set.model.modify(h, |e| {
            e.common.xdata = vec![
                cadaddy_engine::doc::RawTag { code: 1001, value: app.into() },
                cadaddy_engine::doc::RawTag { code: 1000, value: "wall".into() },
                cadaddy_engine::doc::RawTag { code: 1000, value: "LEVEL 1".into() },
            ]
            .into()
        });
    }
    let mut s = Session::new();
    s.open_drawing(set, "Sample Set", None);
    let p = dir.join("set.cdy").to_string_lossy().into_owned();
    s.execute("saveas", &json!({ "path": p })).unwrap();
    let ask = |s: &mut Session| vec![s.execute("bim.tree", &json!({})).unwrap(), s.execute("bim.trades", &json!({})).unwrap()];
    let want = ask(&mut s);
    let mut o = opened_on_demand(&p);
    assert!(!o.needs_whole_set("bim.tree") && !o.needs_whole_set("mep.list"));
    assert_eq!(ask(&mut o), want, "the same model tree with the sheets out of memory");
    assert!(o.has_parked_sheets(), "and nothing was loaded for it");
    let _ = std::fs::remove_dir_all(&dir);
}
