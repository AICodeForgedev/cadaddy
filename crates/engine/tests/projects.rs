//! Q325 / Q5118: the project hub: the list of projects, shelve and restore byte for byte, real
//! open and close of drawings, project scoping, inherited company standards, duplicate / delete /
//! template, and the manifest import (docs/project-manifest.md).

use cadaddy_engine::Session;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn hooks() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
        read_dwf: None,
        write_dwfx: None,
    });
}

/// A folder of the test's own, empty.
fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-projects-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A session whose registry and company library are in `root/config`.
fn session(root: &Path) -> Session {
    hooks();
    let mut s = Session::empty();
    s.project_config = Some(root.join("config"));
    s
}

fn p(path: &Path) -> String {
    path.display().to_string()
}

/// A drawing file with `lines` lines in it.
fn make_cdy(path: &Path, lines: usize) {
    hooks();
    let mut s = Session::new();
    for k in 0..lines {
        s.execute("line", &json!({"points": [[0, k], [10 + k, k]]})).unwrap();
    }
    s.execute("saveas", &json!({"path": p(path)})).unwrap();
}

/// A folder holding one sheet, for a print set to open.
fn make_prints(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    make_cdy(&dir.join("sheet.dxf"), 1);
}

fn entities(path: &Path) -> usize {
    hooks();
    let mut s = Session::new();
    s.execute("open", &json!({"path": p(path)})).unwrap();
    s.doc().unwrap().entity_count()
}

fn fnv(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Every file under `dir` (path relative to it) with its length and hash, caches left out.
fn tree(dir: &Path) -> BTreeMap<String, (usize, u64)> {
    fn walk(base: &Path, cur: &Path, out: &mut BTreeMap<String, (usize, u64)>) {
        for e in std::fs::read_dir(cur).unwrap().flatten() {
            let path = e.path();
            let rel = path.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/");
            if path.is_dir() {
                if !["cache", "tiles", "temp", "indexes"].contains(&rel.as_str()) {
                    walk(base, &path, out);
                }
            } else {
                let data = std::fs::read(&path).unwrap();
                out.insert(rel, (data.len(), fnv(&data)));
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    v.sort();
    v
}

fn bytes_of(dir: &Path) -> u64 {
    fn walk(cur: &Path) -> u64 {
        std::fs::read_dir(cur).unwrap().flatten().map(|e| if e.path().is_dir() { walk(&e.path()) } else { e.metadata().unwrap().len() }).sum()
    }
    walk(dir)
}

fn list(s: &mut Session) -> Vec<Value> {
    s.execute("project.list", &json!({})).unwrap()["projects"].as_array().unwrap().clone()
}

fn entry<'a>(l: &'a [Value], name: &str) -> &'a Value {
    l.iter().find(|e| e["name"] == name).unwrap_or_else(|| panic!("{name} not listed: {l:?}"))
}

// ---- MUST 1: the hub list ----

#[test]
fn project_list_lists_projects_with_status_and_size_and_drawings_moved() {
    let root = scratch("list");
    let mut s = session(&root);
    let mut written: BTreeMap<&str, u64> = BTreeMap::new();
    for (name, lines) in [("Alpha", 3usize), ("Beta", 6), ("Gamma", 9)] {
        let dir = root.join(name);
        s.execute("project.create", &json!({"path": p(&dir), "name": name})).unwrap();
        make_cdy(&dir.join("plan.cdy"), lines);
        std::fs::write(dir.join("notes.txt"), vec![b'x'; 4000 * lines]).unwrap();
        s.execute("project.add", &json!({"path": "plan.cdy", "title": "Plan"})).unwrap();
        written.insert(name, bytes_of(&dir));
    }
    // Shelve Beta (not the open one: Gamma is open).
    s.execute("project.shelve_project", &json!({"path": p(&root.join("Beta"))})).unwrap();

    let l = list(&mut s);
    assert_eq!(l.len(), 3, "{l:?}");
    let (a, b, g) = (entry(&l, "Alpha"), entry(&l, "Beta"), entry(&l, "Gamma"));
    assert_eq!((a["status"].as_str(), b["status"].as_str(), g["status"].as_str()), (Some("active"), Some("shelved"), Some("active")));
    assert_eq!(a["path"].as_str().map(|x| x.ends_with("/Alpha")), Some(true));
    assert_eq!((a["open"].clone(), b["open"].clone(), g["open"].clone()), (json!(false), json!(false), json!(true)));
    // Sizes are real: at least the files written (a project's manifest and registry come on top).
    assert!(a["diskBytes"].as_u64().unwrap() >= written["Alpha"], "{a}");
    assert!(g["diskBytes"].as_u64().unwrap() >= written["Gamma"], "{g}");
    assert!(b["diskBytes"].as_u64().unwrap() > 0 && b["diskBytes"].as_u64().unwrap() < written["Beta"], "shelved is smaller: {b}");
    for e in [a, b, g] {
        assert!(e["lastOpened"].as_str().is_some(), "{e}");
        assert!(e["validationScore"].is_null() && e["validationScoreNote"].as_str().is_some(), "{e}");
        assert!(e.get("drawings").is_none(), "the drawing list moved to project.drawings: {e}");
    }
    assert!(a["openReviewItems"].is_null() && a["openReviewItemsNote"].as_str().is_some());
    // Status filter.
    let shelved = s.execute("project.list", &json!({"status": "shelved"})).unwrap();
    assert_eq!(shelved["total"], 1);
    assert_eq!(shelved["projects"][0]["name"], "Beta");

    // The drawings are asked for by project.drawings (the open project: Gamma).
    let d = s.execute("project.drawings", &json!({})).unwrap();
    assert_eq!(d["total"], 1);
    assert_eq!(d["drawings"][0]["title"], "Plan");
    assert_eq!(d["drawings"][0]["path"], "plan.cdy");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn project_list_survives_a_damaged_registry_and_a_deleted_folder() {
    let root = scratch("list-damaged");
    let mut s = session(&root);
    s.execute("project.create", &json!({"path": p(&root.join("P")), "name": "P"})).unwrap();
    std::fs::remove_dir_all(root.join("P")).unwrap();
    let l = list(&mut s);
    assert_eq!(l.len(), 1);
    assert_eq!(l[0]["status"], "missing");
    std::fs::write(root.join("config/projects.json"), b"{ not json").unwrap();
    assert!(list(&mut s).is_empty());
    let _ = std::fs::remove_dir_all(&root);
}

// ---- MUST 2: shelve compresses, restore is exact ----

#[test]
fn shelve_packs_to_one_archive_and_restore_gives_back_every_file_exactly() {
    let root = scratch("shelve");
    let mut s = session(&root);
    let dir = root.join("Hotel");
    s.execute("project.create", &json!({"path": p(&dir), "name": "Hotel"})).unwrap();
    make_cdy(&dir.join("model.cdy"), 40);
    std::fs::write(dir.join("rules.toml"), "[mounting]\nswitch_height_in = 48\n".repeat(200)).unwrap();
    std::fs::write(dir.join("overrides.json"), json!({"overrides": [{"id": "E-12", "bottom_ft": 10.5}]}).to_string()).unwrap();
    std::fs::create_dir_all(dir.join("review")).unwrap();
    std::fs::write(dir.join("review/items.json"), json!({"items": [{"id": "RV-1", "status": "open"}]}).to_string()).unwrap();
    std::fs::write(dir.join("review/binary.bin"), (0..50_000u32).map(|i| (i * 7 % 251) as u8).collect::<Vec<u8>>()).unwrap();
    for c in ["cache", "tiles", "temp", "indexes"] {
        std::fs::create_dir_all(dir.join(c)).unwrap();
        std::fs::write(dir.join(c).join("blob.bin"), vec![0u8; 300_000]).unwrap();
    }
    s.execute("project.add", &json!({"path": "model.cdy", "title": "Model", "discipline": "Architectural"})).unwrap();

    let before_tree = tree(&dir);
    let before_size = bytes_of(&dir);
    assert!(before_tree.contains_key("project.json") && before_tree.contains_key("review/items.json"));

    let r = s.execute("project.shelve_project", &json!({})).unwrap();
    assert!(s.project.is_none(), "the shelved project is no longer open");
    assert_eq!(names(&dir), vec!["project.json", "project.shelved"], "only the stub and the archive remain");
    let after_size = bytes_of(&dir);
    assert!(after_size < before_size, "{after_size} < {before_size}");
    assert_eq!(r["beforeBytes"], before_size);
    assert_eq!(r["afterBytes"], after_size);
    assert_eq!(r["files"].as_u64(), Some(before_tree.len() as u64));
    let stub: Value = serde_json::from_slice(&std::fs::read(dir.join("project.json")).unwrap()).unwrap();
    assert_eq!((stub["shelved"].clone(), stub["archive"].clone(), stub["name"].clone()), (json!(true), json!("project.shelved"), json!("Hotel")));
    // Shelving twice is refused (nothing to pack).
    assert!(s.execute("project.shelve_project", &json!({"path": p(&dir)})).unwrap_err().to_string().contains("already shelved"));

    let r = s.execute("project.restore_project", &json!({"path": p(&dir)})).unwrap();
    assert_eq!(r["files"].as_u64(), Some(before_tree.len() as u64));
    assert_eq!(tree(&dir), before_tree, "every non-cache file has its length and hash from before the shelve");
    assert_eq!(names(&dir), vec!["model.cdy", "overrides.json", "project.json", "review", "rules.toml"]);
    assert!(!dir.join("cache").exists(), "caches are rebuilt, not restored");
    assert_eq!(s.project.as_ref().map(|p| p.manifest.name.clone()).as_deref(), Some("Hotel"));
    assert_eq!(entities(&dir.join("model.cdy")), 40);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_damaged_archive_is_refused_before_anything_is_written() {
    let root = scratch("shelve-damaged");
    let mut s = session(&root);
    let dir = root.join("P");
    s.execute("project.create", &json!({"path": p(&dir)})).unwrap();
    std::fs::write(dir.join("a.txt"), "hello hello hello hello").unwrap();
    s.execute("project.shelve_project", &json!({})).unwrap();
    let mut a = std::fs::read(dir.join("project.shelved")).unwrap();
    let n = a.len();
    a[n / 2] ^= 0xFF;
    std::fs::write(dir.join("project.shelved"), &a).unwrap();
    let e = s.execute("project.restore_project", &json!({"path": p(&dir)})).unwrap_err().to_string();
    assert!(e.contains("archive") || e.contains("hash") || e.contains("unpack"), "{e}");
    assert_eq!(names(&dir), vec!["project.json", "project.shelved"], "nothing was written");
    // Opening a shelved project unpacks it; the archive may not be hostile either.
    std::fs::write(dir.join("project.shelved"), b"CDYPRJ01").unwrap();
    assert!(s.execute("project.open", &json!({"path": p(&dir)})).is_err());
    std::fs::write(dir.join("project.shelved"), b"garbage").unwrap();
    assert!(s.execute("project.open", &json!({"path": p(&dir)})).is_err());
    let _ = std::fs::remove_dir_all(&root);
}

// ---- MUST 3: activate / shelve / close really load and unload ----

#[test]
fn activate_loads_the_drawing_and_shelve_and_close_save_and_unload_it() {
    let root = scratch("activate");
    let mut s = session(&root);
    let dir = root.join("Job");
    s.execute("project.create", &json!({"path": p(&dir), "name": "Job"})).unwrap();
    make_cdy(&dir.join("a.cdy"), 7);
    make_cdy(&dir.join("b.cdy"), 11);
    s.execute("project.add", &json!({"path": "a.cdy", "title": "Plan A"})).unwrap();
    s.execute("project.add", &json!({"path": "b.cdy", "title": "Plan B"})).unwrap();
    assert!(s.docs.is_empty(), "a project open loads nothing");

    let r = s.execute("project.activate", &json!({"name": "Plan A"})).unwrap();
    assert_eq!(s.docs.len(), 1);
    assert_eq!(s.doc().unwrap().entity_count(), 7);
    assert_eq!(r["entities"], 7);
    assert_eq!(r["activated"]["title"], "Plan A");
    assert!(s.docs[0].path.as_deref().unwrap().ends_with("/a.cdy"));

    // A change in A is saved when B is activated; A is closed.
    s.execute("line", &json!({"points": [[0, 100], [5, 100]]})).unwrap();
    let r = s.execute("project.activate", &json!({"name": "Plan B"})).unwrap();
    assert_eq!(r["shelved"], "Plan A");
    assert_eq!(s.docs.len(), 1, "A was closed");
    assert_eq!(s.doc().unwrap().entity_count(), 11);
    assert_eq!(entities(&dir.join("a.cdy")), 8, "A's edit was saved");
    let ds = s.execute("project.drawings", &json!({})).unwrap();
    assert_eq!((ds["drawings"][0]["status"].clone(), ds["drawings"][1]["status"].clone()), (json!("shelved"), json!("active")));

    // Shelve B: saved (with its edit) and closed.
    s.execute("line", &json!({"points": [[0, 200], [5, 200]]})).unwrap();
    s.execute("line", &json!({"points": [[0, 300], [5, 300]]})).unwrap();
    let r = s.execute("project.shelve", &json!({})).unwrap();
    assert_eq!((r["shelved"].clone(), r["saved"].clone()), (json!("Plan B"), json!(true)));
    assert!(s.docs.is_empty());
    assert_eq!(entities(&dir.join("b.cdy")), 13);

    // Close: the drawing and the project's state are gone.
    s.execute("project.activate", &json!({"name": "Plan A"})).unwrap();
    make_prints(&dir.join("prints"));
    s.execute("printset.open", &json!({"path": p(&dir.join("prints")), "name": "Set"})).unwrap();
    assert!(s.printset_project.is_some());
    let r = s.execute("project.close", &json!({})).unwrap();
    assert_eq!(r["closed"].as_array().map(Vec::len), Some(1), "{r}");
    assert!(s.docs.is_empty() && s.project.is_none() && s.printset_project.is_none());
    assert!(s.execute("project.info", &json!({})).is_err());
    // A drawing outside the project stays open through close.
    make_cdy(&root.join("elsewhere.cdy"), 2);
    s.execute("open", &json!({"path": p(&root.join("elsewhere.cdy"))})).unwrap();
    s.execute("project.open", &json!({"path": p(&dir)})).unwrap();
    s.execute("project.activate", &json!({"name": "Plan A"})).unwrap();
    assert_eq!(s.docs.len(), 2);
    s.execute("project.close", &json!({})).unwrap();
    assert_eq!(s.docs.len(), 1);
    assert!(s.docs[0].path.as_deref().unwrap().ends_with("elsewhere.cdy"));
    let _ = std::fs::remove_dir_all(&root);
}

// ---- MUST 4: project scoping ----

fn note_drawing(path: &Path, note: &str, rule: &str) {
    hooks();
    let mut s = Session::new();
    s.execute("project.note", &json!({"text": note})).unwrap();
    s.execute("project.rules.set", &json!({"domain": "custom", "key": "switch_label", "value": rule, "reason": "test setup"})).unwrap();
    s.execute("saveas", &json!({"path": p(path)})).unwrap();
}

#[test]
fn briefing_printset_and_rules_belong_to_the_open_project() {
    let root = scratch("scope");
    let mut s = session(&root);
    for (name, note, rule, set) in [("Alpha", "alpha handoff", "A-48", "Alpha Set"), ("Beta", "beta handoff", "B-44", "Beta Set")] {
        let dir = root.join(name);
        s.execute("project.create", &json!({"path": p(&dir), "name": name})).unwrap();
        note_drawing(&dir.join("plan.cdy"), note, rule);
        make_prints(&dir.join("prints"));
        s.execute("project.add", &json!({"path": "plan.cdy", "title": "Plan"})).unwrap();
        let _ = set;
    }
    s.execute("project.close", &json!({})).unwrap();

    // Open A: its briefing, rules and print set.
    s.execute("project.open", &json!({"path": p(&root.join("Alpha"))})).unwrap();
    s.execute("project.activate", &json!({"name": "Plan"})).unwrap();
    s.execute("printset.open", &json!({"path": p(&root.join("Alpha/prints")), "name": "Alpha Set"})).unwrap();
    let ba = s.execute("project.briefing", &json!({})).unwrap();
    let da = s.execute("printset.dashboard", &json!({})).unwrap();
    let ra = s.execute("project.rules.get", &json!({"domain": "custom", "key": "switch_label"})).unwrap();
    assert_eq!((ba["project"].clone(), da["project"].clone()), (json!("Alpha"), json!("Alpha")));
    assert_eq!(ba["notes"][0]["text"], "alpha handoff");
    assert_eq!(da["name"], "Alpha Set");
    assert!(ra.to_string().contains("A-48"), "{ra}");

    // Open B in the same session: A is closed and nothing of it remains.
    s.execute("project.open", &json!({"path": p(&root.join("Beta"))})).unwrap();
    assert!(s.docs.is_empty() && s.printset_project.is_none(), "A's drawing and print set are gone");
    assert!(s.execute("printset.dashboard", &json!({})).unwrap_err().to_string().contains("no print-set project open"));
    s.execute("project.activate", &json!({"name": "Plan"})).unwrap();
    s.execute("printset.open", &json!({"path": p(&root.join("Beta/prints")), "name": "Beta Set"})).unwrap();
    let bb = s.execute("project.briefing", &json!({})).unwrap();
    let db = s.execute("printset.dashboard", &json!({})).unwrap();
    let rb = s.execute("project.rules.get", &json!({"domain": "custom", "key": "switch_label"})).unwrap();
    assert_eq!((bb["project"].clone(), db["project"].clone()), (json!("Beta"), json!("Beta")));
    assert_eq!(bb["notes"][0]["text"], "beta handoff");
    assert_eq!(db["name"], "Beta Set");
    assert!(rb.to_string().contains("B-44") && !rb.to_string().contains("A-48"), "{rb}");
    for v in [&bb, &db, &rb] {
        let t = v.to_string();
        assert!(!t.contains("alpha") && !t.contains("Alpha") && !t.contains("A-48"), "nothing from A: {t}");
    }
    assert_ne!(ba["notes"], bb["notes"]);
    // Print-set answers all name the project.
    assert_eq!(s.execute("printset.sheets", &json!({})).unwrap()["project"], "Beta");
    let _ = std::fs::remove_dir_all(&root);
}

// ---- MUST 5: company standards inheritance ----

fn effective(s: &mut Session, domain: &str, key: &str) -> Value {
    let l = s.execute("project.standards.list", &json!({"domain": domain})).unwrap();
    l["standards"].as_array().unwrap().iter().find(|e| e["key"] == key).cloned().unwrap_or(Value::Null)
}

#[test]
fn projects_inherit_company_standards_and_override_them() {
    let root = scratch("standards");
    let mut s = session(&root);
    s.execute("project.company.set", &json!({"domain": "mounting", "key": "switch_height_in", "value": 48})).unwrap();
    s.execute("project.company.set", &json!({"domain": "mounting", "key": "receptacle_height_in", "value": 16})).unwrap();
    for n in ["A", "B"] {
        s.execute("project.create", &json!({"path": p(&root.join(n)), "name": n})).unwrap();
    }
    s.execute("project.open", &json!({"path": p(&root.join("A"))})).unwrap();
    s.execute("project.standards.set", &json!({"domain": "mounting", "key": "switch_height_in", "value": 44})).unwrap();
    assert_eq!(
        effective(&mut s, "mounting", "switch_height_in"),
        json!({"domain": "mounting", "key": "switch_height_in", "value": 44, "source": "project", "overrides": {"source": "company", "value": 48}})
    );
    assert_eq!(
        effective(&mut s, "mounting", "receptacle_height_in"),
        json!({"domain": "mounting", "key": "receptacle_height_in", "value": 16, "source": "company"})
    );

    s.execute("project.open", &json!({"path": p(&root.join("B"))})).unwrap();
    assert_eq!(
        effective(&mut s, "mounting", "switch_height_in"),
        json!({"domain": "mounting", "key": "switch_height_in", "value": 48, "source": "company"})
    );

    // The company changes: B follows, A keeps its own.
    s.execute("project.company.set", &json!({"domain": "mounting", "key": "switch_height_in", "value": 46})).unwrap();
    assert_eq!(effective(&mut s, "mounting", "switch_height_in")["value"], 46);
    assert_eq!(effective(&mut s, "mounting", "switch_height_in")["source"], "company");
    s.execute("project.open", &json!({"path": p(&root.join("A"))})).unwrap();
    let a = effective(&mut s, "mounting", "switch_height_in");
    assert_eq!((a["value"].clone(), a["source"].clone(), a["overrides"]["value"].clone()), (json!(44), json!("project"), json!(46)));

    // Dropping the override gives the company value back; the list is the effective set.
    s.execute("project.standards.remove", &json!({"domain": "mounting", "key": "switch_height_in"})).unwrap();
    assert_eq!(
        effective(&mut s, "mounting", "switch_height_in"),
        json!({"domain": "mounting", "key": "switch_height_in", "value": 46, "source": "company"})
    );
    let all = s.execute("project.standards.list", &json!({})).unwrap();
    assert_eq!(all["total"], 2);
    // The library is a file in the settings folder, the same for every session.
    let mut t = session(&root);
    assert_eq!(t.execute("project.company.list", &json!({})).unwrap()["total"], 2);
    assert!(t.execute("project.company.remove", &json!({"domain": "mounting", "key": "nope"})).is_err());
    let _ = std::fs::remove_dir_all(&root);
}

// ---- MUST 6: duplicate, delete, template ----

#[test]
fn duplicate_gives_an_independent_copy() {
    let root = scratch("duplicate");
    let mut s = session(&root);
    let src = root.join("Src");
    s.execute("project.create", &json!({"path": p(&src), "name": "Src"})).unwrap();
    make_cdy(&src.join("plan.cdy"), 5);
    std::fs::write(src.join("rules.toml"), "[a]\nb = 1\n").unwrap();
    std::fs::create_dir_all(src.join("cache")).unwrap();
    std::fs::write(src.join("cache/x.bin"), b"cache").unwrap();
    s.execute("project.add", &json!({"path": "plan.cdy", "title": "Plan"})).unwrap();
    s.execute("project.standards.set", &json!({"domain": "mounting", "key": "switch_height_in", "value": 48})).unwrap();
    let src_before = tree(&src);

    let dup = root.join("Copy");
    let r = s.execute("project.duplicate", &json!({"path": p(&src), "to": p(&dup), "name": "Copy"})).unwrap();
    assert_eq!(r["project"]["name"], "Copy");
    assert_eq!(tree(&src), src_before, "the source is untouched");
    assert!(!dup.join("cache").exists());
    let mut expect = src_before.clone();
    let (dm, _) = expect.remove("project.json").unwrap();
    let mut got = tree(&dup);
    let (gm, _) = got.remove("project.json").unwrap();
    assert!(dm > 0 && gm > 0);
    assert_eq!(got, expect, "same files");
    let l = list(&mut s);
    assert_eq!(l.len(), 2);
    assert_eq!(entry(&l, "Copy")["status"], "active");

    // Edit the copy; the original does not move.
    s.execute("project.open", &json!({"path": p(&dup)})).unwrap();
    s.execute("project.standards.set", &json!({"domain": "mounting", "key": "switch_height_in", "value": 44})).unwrap();
    s.execute("project.activate", &json!({"name": "Plan"})).unwrap();
    s.execute("line", &json!({"points": [[0, 50], [9, 50]]})).unwrap();
    s.execute("project.close", &json!({})).unwrap();
    std::fs::write(dup.join("rules.toml"), "[a]\nb = 2\n").unwrap();
    assert_eq!(tree(&src), src_before);
    assert_eq!(entities(&src.join("plan.cdy")), 5);
    assert_eq!(entities(&dup.join("plan.cdy")), 6);
    s.execute("project.open", &json!({"path": p(&src)})).unwrap();
    assert_eq!(effective(&mut s, "mounting", "switch_height_in")["value"], 48);
    // Into itself or onto files: refused.
    assert!(s.execute("project.duplicate", &json!({"to": p(&src.join("inner"))})).is_err());
    assert!(s.execute("project.duplicate", &json!({"to": p(&dup)})).is_err());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn delete_needs_confirm_and_then_removes_folder_and_list_entry() {
    let root = scratch("delete");
    let mut s = session(&root);
    let dir = root.join("Doomed");
    s.execute("project.create", &json!({"path": p(&dir), "name": "Doomed"})).unwrap();
    std::fs::write(dir.join("keep.txt"), "important").unwrap();
    s.execute("project.create", &json!({"path": p(&root.join("Other")), "name": "Other"})).unwrap();
    let before = tree(&dir);
    for params in [json!({"path": p(&dir)}), json!({"path": p(&dir), "confirm": false}), json!({"path": p(&dir), "confirm": "yes"})] {
        let e = s.execute("project.delete", &params).unwrap_err().to_string();
        assert!(e.contains("confirm: true"), "the refusal says how: {e}");
        assert_eq!(tree(&dir), before, "folder intact");
        assert_eq!(list(&mut s).len(), 2);
    }
    // A folder that is not a project is never deleted.
    let plain = root.join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    std::fs::write(plain.join("f"), "x").unwrap();
    assert!(s.execute("project.delete", &json!({"path": p(&plain), "confirm": true})).is_err());
    assert!(plain.join("f").exists());

    let r = s.execute("project.delete", &json!({"path": p(&dir), "confirm": true})).unwrap();
    assert_eq!(r["deleted"], "Doomed");
    assert!(!dir.exists());
    let l = list(&mut s);
    assert_eq!(l.iter().map(|e| e["name"].clone()).collect::<Vec<_>>(), vec![json!("Other")]);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_project_from_a_template_gets_standards_and_rules_not_drawings() {
    let root = scratch("template");
    let mut s = session(&root);
    let t = root.join("Template");
    s.execute("project.create", &json!({"path": p(&t), "name": "Template"})).unwrap();
    s.execute("project.standards.set", &json!({"domain": "mounting", "key": "switch_height_in", "value": 48})).unwrap();
    s.execute("project.standards.set", &json!({"domain": "naming", "key": "room_label", "value": "Room {room}"})).unwrap();
    std::fs::write(t.join("rules.toml"), "[mounting]\nswitch_height_in = 48\n").unwrap();
    std::fs::create_dir_all(t.join("types")).unwrap();
    std::fs::write(t.join("types/walls.json"), json!({"types": ["W1"]}).to_string()).unwrap();
    make_cdy(&t.join("sheet.cdy"), 3);
    s.execute("project.add", &json!({"path": "sheet.cdy", "title": "Sheet"})).unwrap();

    let n = root.join("NewJob");
    let r = s.execute("project.create", &json!({"path": p(&n), "name": "New Job", "template": p(&t)})).unwrap();
    assert_eq!(r["project"]["name"], "New Job");
    assert_eq!(r["project"]["drawings"], 0);
    assert_eq!(names(&n), vec!["project.json", "rules.toml", "types"]);
    assert_eq!(std::fs::read(n.join("rules.toml")).unwrap(), std::fs::read(t.join("rules.toml")).unwrap());
    assert_eq!(std::fs::read(n.join("types/walls.json")).unwrap(), std::fs::read(t.join("types/walls.json")).unwrap());
    let l = s.execute("project.standards.list", &json!({})).unwrap();
    let got: Vec<(String, String, Value)> = l["standards"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (e["domain"].as_str().unwrap().into(), e["key"].as_str().unwrap().into(), e["value"].clone()))
        .collect();
    assert_eq!(
        got,
        vec![
            ("mounting".to_string(), "switch_height_in".to_string(), json!(48)),
            ("naming".to_string(), "room_label".to_string(), json!("Room {room}")),
        ]
    );
    assert_eq!(s.execute("project.drawings", &json!({})).unwrap()["total"], 0);
    // The template is not changed, and a shelved or missing template is refused.
    assert!(t.join("sheet.cdy").exists());
    assert!(s.execute("project.create", &json!({"path": p(&root.join("X")), "template": p(&root.join("nope"))})).is_err());
    let _ = std::fs::remove_dir_all(&root);
}

// ---- MUST 7: manifest import ----

fn write_manifest(dir: &Path, files: Value) {
    std::fs::write(dir.join("manifest.json"), json!({"name": "Transmittal", "files": files}).to_string()).unwrap();
}

fn import(s: &mut Session, dir: &Path) -> Value {
    s.execute("project.import_manifest", &json!({"path": p(dir)})).unwrap()
}

#[test]
fn reimport_reports_what_changed_by_file_and_queues_only_those() {
    let root = scratch("manifest");
    let mut s = session(&root);
    let dir = root.join("Docs");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("rfi-12.pdf"), b"rfi twelve").unwrap();
    std::fs::write(dir.join("sub-03.pdf"), b"submittal three").unwrap();
    std::fs::write(dir.join("a-101.pdf"), b"sheet a101").unwrap();
    let files = |rfi: &str, sub: &str, with_sheet: bool, ghost: bool| {
        let mut f = vec![
            json!({"path": "rfi-12.pdf", "kind": "rfi", "status": rfi, "title": "RFI 12", "date": "2026-10-01"}),
            json!({"path": "sub-03.pdf", "kind": "submittal", "status": sub, "revision": "1"}),
        ];
        if with_sheet {
            f.push(json!({"path": "a-101.pdf", "kind": "sheet", "discipline": "Architectural", "revision": "A"}));
        }
        if ghost {
            f.push(json!({"path": "ghost.pdf", "kind": "spec"}));
        }
        Value::Array(f)
    };
    write_manifest(&dir, files("Open", "Approved as Noted", true, false));
    let r = import(&mut s, &dir);
    assert_eq!(r["files"]["added"], json!(["rfi-12.pdf", "sub-03.pdf", "a-101.pdf"]));
    assert_eq!((r["files"]["changed"].clone(), r["files"]["removed"].clone(), r["files"]["missing"].clone()), (json!([]), json!([]), json!([])));
    assert_eq!(r["queued"], json!(["rfi-12.pdf", "sub-03.pdf", "a-101.pdf"]));
    let d = s.execute("project.drawings", &json!({})).unwrap();
    let by = |d: &Value, path: &str| d["drawings"].as_array().unwrap().iter().find(|x| x["path"] == path).cloned().unwrap();
    assert_eq!(by(&d, "rfi-12.pdf")["reviewStatus"], "open");
    assert_eq!(by(&d, "sub-03.pdf")["reviewStatus"], "approved as noted", "stored in its own field, canonical spelling");
    assert_eq!(by(&d, "sub-03.pdf")["notes"], "", "not in notes");

    // Same manifest again: nothing changed, nothing new queued.
    let r = import(&mut s, &dir);
    assert_eq!((r["added"].clone(), r["changed"].clone(), r["removed"].clone(), r["queued"].clone()), (json!(0), json!(0), json!(0), json!([])));

    // Only the RFI's status changes; the bytes are identical.
    write_manifest(&dir, files("answered", "Approved as Noted", true, false));
    let r = import(&mut s, &dir);
    assert_eq!(r["files"]["changed"], json!([{"path": "rfi-12.pdf", "fields": ["status"]}]));
    assert_eq!((r["files"]["added"].clone(), r["files"]["removed"].clone()), (json!([]), json!([])));
    assert_eq!(r["queued"], json!(["rfi-12.pdf"]));
    let d = s.execute("project.drawings", &json!({})).unwrap();
    assert_eq!(by(&d, "rfi-12.pdf")["reviewStatus"], "answered");

    // The file changes on disk and a revision label moves; another entry is dropped; one is
    // listed but missing.
    std::fs::write(dir.join("sub-03.pdf"), b"submittal three, resubmitted").unwrap();
    write_manifest(&dir, files("answered", "Revise and Resubmit", false, true));
    let r = import(&mut s, &dir);
    assert_eq!(r["files"]["changed"], json!([{"path": "sub-03.pdf", "fields": ["content", "status"]}]));
    assert_eq!(r["files"]["removed"], json!(["a-101.pdf"]));
    assert_eq!(r["files"]["added"], json!(["ghost.pdf"]));
    assert_eq!(r["files"]["missing"], json!(["ghost.pdf"]));
    let mut q: Vec<String> = r["queued"].as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect();
    q.sort();
    assert_eq!(q, vec!["ghost.pdf", "sub-03.pdf"]);
    let info = s.execute("project.info", &json!({})).unwrap();
    let mut queue: Vec<String> = info["project"]["queue"].as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect();
    queue.sort();
    assert_eq!(queue, vec!["ghost.pdf", "rfi-12.pdf", "sub-03.pdf"], "the removed sheet left the queue");
    let d = s.execute("project.drawings", &json!({})).unwrap();
    let paths: Vec<&str> = d["drawings"].as_array().unwrap().iter().map(|x| x["path"].as_str().unwrap()).collect();
    assert_eq!(paths, vec!["rfi-12.pdf", "sub-03.pdf", "ghost.pdf"]);

    // A re-import in a new session finds the folder's project and its earlier state.
    let mut t = session(&root);
    let r = import(&mut t, &dir);
    assert_eq!((r["added"].clone(), r["changed"].clone(), r["removed"].clone()), (json!(0), json!(0), json!(0)));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn manifest_status_is_validated_per_kind_and_paths_cannot_leave_the_folder() {
    let root = scratch("manifest-valid");
    let mut s = session(&root);
    let dir = root.join("Docs");
    std::fs::create_dir_all(&dir).unwrap();
    let errors = |s: &mut Session, files: Value| -> Vec<String> {
        write_manifest(&dir, files);
        let r = s.execute("project.import_manifest", &json!({"path": p(&dir)})).unwrap();
        assert_eq!(r["valid"], json!(false), "{r}");
        r["errors"].as_array().unwrap().iter().map(|e| e.as_str().unwrap().to_string()).collect()
    };
    for ok in ["approved", "APPROVED AS NOTED", "revise_and_resubmit", "Rejected"] {
        write_manifest(&dir, json!([{"path": "s.pdf", "kind": "submittal", "status": ok}]));
        assert!(s.execute("project.validate_manifest", &json!({"path": p(&dir.join("manifest.json"))})).unwrap()["valid"] == json!(true), "{ok}");
    }
    let e = errors(&mut s, json!([{"path": "r.pdf", "kind": "rfi", "status": "approved"}]));
    assert_eq!(e, vec!["files[0].status `approved` is not one of: open / answered"]);
    let e = errors(&mut s, json!([{"path": "s.pdf", "kind": "submittal", "status": "maybe"}]));
    assert_eq!(e, vec!["files[0].status `maybe` is not one of: approved / approved as noted / revise and resubmit / rejected"]);
    let e = errors(&mut s, json!([{"path": "d.pdf", "kind": "sheet", "status": "approved"}]));
    assert_eq!(e, vec!["files[0].status is only used for submittals and RFIs (this entry is a `sheet`)"]);
    let e = errors(&mut s, json!([{"path": "r.pdf", "kind": "rfi", "status": 3}]));
    assert_eq!(e, vec!["files[0].status must be a string"]);
    // Paths that leave the folder.
    for bad in ["../x", "a/../../x", "/etc/passwd", "C:/x", "a\\b", "./a", "a//b", ""] {
        let e = errors(&mut s, json!([{"path": bad}]));
        assert_eq!(e.len(), 1, "{bad}: {e:?}");
        assert!(e[0].contains("path"), "{bad}: {e:?}");
    }
    assert!(s.project.is_none(), "nothing was imported");
    assert!(!dir.join("project.json").exists());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn hostile_manifest_json_never_panics() {
    let root = scratch("manifest-hostile");
    let mut s = session(&root);
    let dir = root.join("Docs");
    std::fs::create_dir_all(&dir).unwrap();
    let huge = format!("{{\"files\": [{}]}}", vec![r#"{"path": "a"}"#; 10_001].join(","));
    let deep = format!("{}{}", "[".repeat(5000), "]".repeat(5000));
    let cases: Vec<(&str, String)> = vec![
        ("truncated", r#"{"name": "x", "files": [{"path": "a.p"#.to_string()),
        ("empty", String::new()),
        ("array", "[1,2,3]".into()),
        ("files wrong type", r#"{"files": "nope"}"#.into()),
        ("files items wrong", r#"{"files": [1, null, "x", [], {"path": 5}, {"path": null}, {"path": ["a"]}]}"#.into()),
        (
            "fields wrong",
            r#"{"name": 7, "files": [{"path": "a", "title": 1, "revision": [], "date": {}, "kind": 9, "discipline": true, "links": 4}]}"#.into(),
        ),
        ("links wrong", r#"{"files": [{"path": "a", "links": {"rfi": [1, 2], "x": "s", "y": {"z": 1}}}]}"#.into()),
        (
            "numeric status",
            r#"{"files": [{"path": "a", "kind": "rfi", "status": 1}, {"path": "b", "status": 2.5}, {"path": "c", "status": null}]}"#.into(),
        ),
        ("huge", huge),
        ("deep", deep),
        ("long text", format!(r#"{{"files": [{{"path": "a", "title": "{}"}}]}}"#, "é".repeat(5000))),
        ("nul and controls", "{\"files\": [{\"path\": \"a\\u0000b\", \"title\": \"\\u0007\"}]}".into()),
    ];
    for (name, body) in &cases {
        std::fs::write(dir.join("manifest.json"), body).unwrap();
        for cmd in ["project.import_manifest", "project.validate_manifest"] {
            let r = s.execute(cmd, &json!({"path": p(&dir)}));
            let r2 = s.execute(cmd, &json!({"path": p(&dir.join("manifest.json"))}));
            for r in [r, r2] {
                match r {
                    Ok(v) => assert_eq!(v["valid"], json!(false), "{name} {cmd}: {v}"),
                    Err(e) => assert!(!e.to_string().is_empty(), "{name}"),
                }
            }
        }
    }
    // Missing folder, directory instead of file.
    assert!(s.execute("project.import_manifest", &json!({"path": p(&root.join("nope"))})).is_err());
    std::fs::remove_file(dir.join("manifest.json")).unwrap();
    std::fs::create_dir(dir.join("manifest.json")).unwrap();
    assert!(s.execute("project.import_manifest", &json!({"path": p(&dir)})).is_err());
    let _ = std::fs::remove_dir_all(&root);
}
