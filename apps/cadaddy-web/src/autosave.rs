//! Autosave in the browser: the engine's store autosave ([`cadaddy_engine::autosave::store`])
//! kept in the page's local storage. Drawings a closed, crashed or reloaded tab left unsaved are
//! offered back by Drawing Recovery on the next visit.

use web_sys::js_sys;

fn local() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

fn get(key: &str) -> Option<String> {
    local()?.get_item(key).ok().flatten()
}

/// `false` when the browser refuses (storage full or blocked).
fn set(key: &str, value: &str) -> bool {
    local().is_some_and(|s| s.set_item(key, value).is_ok())
}

fn remove(key: &str) {
    if let Some(s) = local() {
        let _ = s.remove_item(key);
    }
}

fn keys() -> Vec<String> {
    let Some(s) = local() else { return Vec::new() };
    let n = s.length().unwrap_or(0).min(10_000);
    (0..n).filter_map(|i| s.key(i).ok().flatten()).collect()
}

fn now() -> f64 {
    js_sys::Date::now() / 1000.0
}

/// Install browser storage as the autosave store and turn autosave on for `app`.
pub fn enable(app: &mut cadaddy_ui_egui::CadApp) {
    if local().is_none() {
        log::warn!("autosave: the browser's local storage is not available");
        return;
    }
    cadaddy_engine::autosave::store::set_store(cadaddy_engine::autosave::store::Store { get, set, remove, keys, now });
    let random = (js_sys::Math::random() * 1e9) as u32;
    app.enable_store_autosave(cadaddy_engine::autosave::store::instance_id(random, now()));
}
