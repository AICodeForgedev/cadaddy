//! Autosave into a key/value store (the web build's browser storage): drawings are written
//! SAVETIME after a change, a tab's lock entry keeps its autosaves out of recovery while it runs,
//! and the autosaves of a tab that stopped are offered by Drawing Recovery and open as copies.
//! The store here is in memory, with a clock the test moves.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use cadaddy_engine::Session;
use cadaddy_engine::autosave::store::{self, Store};
use cadaddy_engine::autosave::{self, Autosaver};
use cadaddy_engine::cmd::file::{IoHooks, set_io};
use serde_json::json;

static MAP: Mutex<BTreeMap<String, String>> = Mutex::new(BTreeMap::new());
static NOW: AtomicU64 = AtomicU64::new(1_000_000);
/// The tests share the store and the clock: they take turns.
static TURN: Mutex<()> = Mutex::new(());

fn get(k: &str) -> Option<String> {
    MAP.lock().unwrap().get(k).cloned()
}
fn set(k: &str, v: &str) -> bool {
    MAP.lock().unwrap().insert(k.into(), v.into());
    true
}
fn remove(k: &str) {
    MAP.lock().unwrap().remove(k);
}
fn keys() -> Vec<String> {
    MAP.lock().unwrap().keys().cloned().collect()
}
fn now() -> f64 {
    NOW.load(Ordering::Relaxed) as f64
}
fn advance(secs: u64) {
    NOW.fetch_add(secs, Ordering::Relaxed);
}

fn setup() -> std::sync::MutexGuard<'static, ()> {
    let turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
    set_io(IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
    });
    store::set_store(Store { get, set, remove, keys, now });
    autosave::set_savetime(1);
    MAP.lock().unwrap().clear();
    turn
}

fn autosaves() -> Vec<String> {
    keys().into_iter().filter(|k| k.starts_with("cadaddy.autosave/")).collect()
}

fn draw(s: &mut Session) {
    s.execute("circle", &json!({ "center": [0, 0], "radius": 2 })).unwrap();
}

#[test]
fn changes_are_stored_savetime_after_and_removed_when_saved_or_closed() {
    let _turn = setup();
    let mut s = Session::new();
    let mut a = Autosaver::new(store::instance_id(7, now()));
    draw(&mut s);
    assert_eq!(a.tick_store(&s), 0, "the change starts SAVETIME's clock");
    advance(30);
    assert_eq!(a.tick_store(&s), 0, "not a minute yet");
    advance(31);
    assert_eq!(a.tick_store(&s), 1);
    let saved = autosaves();
    assert_eq!(saved.len(), 1);
    let v: serde_json::Value = serde_json::from_str(&get(&saved[0]).unwrap()).unwrap();
    assert!(v["dxf"].as_str().unwrap().contains("CIRCLE") && v["drawing"] == "Drawing1", "{v}");
    assert!(keys().iter().any(|k| k == &format!("cadaddy.autosave.lock/{}", a.instance)), "the lock is kept fresh");
    // Unchanged: nothing more is written.
    advance(120);
    assert_eq!(a.tick_store(&s), 0);
    // Saved (no unsaved changes): its autosave goes.
    let st = s.state_mut().unwrap();
    st.saved = st.doc.clone();
    a.tick_store(&s);
    assert!(autosaves().is_empty());
}

#[test]
fn a_stopped_tabs_autosaves_are_recovered_as_copies() {
    let _turn = setup();
    let mut s = Session::new();
    let mut a = Autosaver::new(store::instance_id(11, now()));
    draw(&mut s);
    draw(&mut s);
    a.tick_store(&s);
    advance(61);
    a.tick_store(&s);
    // While the tab runs (fresh lock) nothing is offered, to it or to another tab.
    assert!(store::recoverable_store(&a.instance).is_empty());
    assert!(store::recoverable_store("99-1").is_empty());
    // The tab stops: once its lock is stale, another tab is offered the drawing.
    advance(autosave::STALE_LOCK_SECS + 1);
    let items = store::recoverable_store("99-1");
    assert_eq!(items.len(), 1);
    assert!(items[0].path.starts_with(store::SCHEME) && items[0].drawing == "Drawing1" && items[0].bytes > 0, "{:?}", items[0]);

    // Drawing Recovery lists and opens it as an unsaved copy with its objects.
    let mut s2 = Session::new();
    let listed = s2.execute("drawingrecovery", &json!({})).unwrap();
    assert_eq!(listed["files"][0]["path"], items[0].path.as_str());
    let r = s2.execute("drawingrecovery", &json!({ "action": "open", "path": items[0].path })).unwrap();
    assert!(r["uid"].is_u64());
    let st = s2.state().unwrap();
    assert_eq!((st.title.as_str(), st.path.as_deref()), ("Drawing1", None));
    assert_eq!(st.doc.model.iter().filter(|e| e.kind.type_name() == "Circle").count(), 2);
    // Discarding removes it.
    s2.execute("drawingrecovery", &json!({ "action": "discard", "path": items[0].path })).unwrap();
    assert!(autosaves().is_empty());
    assert!(store::read_store(&items[0].path).is_none());
}

#[test]
fn closing_normally_removes_the_tabs_autosaves_and_stale_locks_are_pruned() {
    let _turn = setup();
    let mut s = Session::new();
    let mut a = Autosaver::new(store::instance_id(3, now()));
    draw(&mut s);
    a.tick_store(&s);
    advance(61);
    a.tick_store(&s);
    assert_eq!(autosaves().len(), 1);
    a.shutdown_store();
    assert!(keys().is_empty(), "{:?}", keys());

    // A lock left without autosaves is removed once stale; one that is fresh stays.
    set("cadaddy.autosave.lock/5-1", &format!("{}", now() as u64));
    store::prune_store_locks();
    assert_eq!(keys().len(), 1);
    advance(autosave::STALE_LOCK_SECS + 1);
    store::prune_store_locks();
    assert!(keys().is_empty());
    // Only CADaddy's own names are read or removed.
    set("someone-else", "x");
    store::discard_store("browser:someone-else");
    assert!(store::read_store("browser:someone-else").is_none());
    assert_eq!(keys(), vec!["someone-else".to_string()]);
}
