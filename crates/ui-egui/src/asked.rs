//! What an open palette or dialog shows, worked out once per state of the drawing and kept
//! between frames (Q6573). The RFI List, Element Tags, MEP Review and the Layer Properties Manager
//! read their whole list again on every frame (70-400 ms a frame on a large model); now a frame
//! asks the engine only when the drawing (or the dialog's own filter) has changed since, and lays
//! out only the rows on screen ([`rows_in_view`]).

use std::sync::{Arc, Weak};

use cadaddy_engine::doc::Drawing;
use serde_json::Value;

use crate::CadApp;

/// A state of the active drawing: which drawing, its revision, and the drawing itself, held
/// weakly. A change the revision doesn't count (a list command that fills in ids) still makes a
/// new drawing: with a weak pointer to it about, `Arc::make_mut` moves even an unshared drawing
/// to a new place, and the old place can't be taken by another while the stamp names it.
#[derive(Clone, Debug)]
pub struct Stamp {
    uid: u64,
    revision: u64,
    doc: Weak<Drawing>,
}

impl PartialEq for Stamp {
    fn eq(&self, o: &Self) -> bool {
        self.uid == o.uid && self.revision == o.revision && self.doc.ptr_eq(&o.doc)
    }
}

/// The active drawing's state now (`None`: no drawing open).
pub fn stamp(app: &CadApp) -> Option<Stamp> {
    app.session.state().ok().map(|st| Stamp { uid: st.uid, revision: st.revision, doc: Arc::downgrade(&st.doc) })
}

/// What [`kept`] keeps under one id.
struct Kept<K, T> {
    stamp: Option<Stamp>,
    key: K,
    value: Arc<T>,
}

impl<K: Clone, T> Clone for Kept<K, T> {
    fn clone(&self) -> Self {
        Self { stamp: self.stamp.clone(), key: self.key.clone(), value: Arc::clone(&self.value) }
    }
}

/// The value `make` works out of the drawing for `key` (a dialog's filter), kept under `id` while
/// the drawing and `key` stay as they are: shared, so a frame that reads it copies a pointer.
/// `make` may change the drawing (a list that fills in ids): what is kept is for the state it
/// left.
pub fn kept<K, T>(app: &mut CadApp, ctx: &egui::Context, id: egui::Id, key: &K, make: impl FnOnce(&mut CadApp) -> T) -> Arc<T>
where
    K: Clone + PartialEq + Send + Sync + 'static,
    T: Send + Sync + 'static,
{
    let now = stamp(app);
    if let Some(k) = ctx.data(|d| d.get_temp::<Kept<K, T>>(id))
        && k.stamp == now
        && k.key == *key
    {
        return k.value;
    }
    let value = Arc::new(make(app));
    let k = Kept { stamp: stamp(app), key: key.clone(), value: Arc::clone(&value) };
    ctx.data_mut(|d| d.insert_temp(id, k));
    value
}

/// The reply of command `cmd` with `params`, asked once per state of the drawing and of
/// `params` (`Null` when it fails, as when no drawing is open).
pub fn ask(app: &mut CadApp, ctx: &egui::Context, cmd: &'static str, params: &Value) -> Arc<Value> {
    kept(app, ctx, egui::Id::new(("asked", cmd)), params, |app| app.session.execute(cmd, params).unwrap_or(Value::Null))
}

/// The heights the rows of a [`rows_in_view`] list had when last laid out, for one key.
#[derive(Clone, Debug, Default)]
struct Heights<K> {
    key: Option<K>,
    at: Vec<f32>,
}

/// A list of `n` rows of varying height in a scroll area (`viewport`: what is on screen, in the
/// content's coordinates, as `ScrollArea::show_viewport` gives it): lays out only the rows that
/// reach onto the screen and passes over the others by the height each had when last laid out
/// (one line's until then), as long as `key` (what the rows show) stays the same. So a frame
/// costs what is on screen, not how many rows there are (Q6542, Q6573). Each row gets an id of
/// its own, so a widget's state (a text being edited) stays with its row as the list scrolls.
pub fn rows_in_view<K>(ui: &mut egui::Ui, viewport: egui::Rect, id: egui::Id, key: &K, n: usize, mut row: impl FnMut(&mut egui::Ui, usize))
where
    K: Clone + PartialEq + Default + Send + Sync + 'static,
{
    let line = ui.spacing().interact_size.y + ui.spacing().item_spacing.y;
    let mut h = ui.data_mut(|d| std::mem::take(d.get_temp_mut_or_default::<Heights<K>>(id)));
    if h.key.as_ref() != Some(key) {
        h = Heights { key: Some(key.clone()), at: Vec::new() };
    }
    h.at.resize(n, line);
    let top = ui.max_rect().top();
    let mut y = ui.cursor().min.y - top;
    let mut first = 0;
    for &hi in &h.at {
        if y + hi >= viewport.min.y {
            break;
        }
        y += hi;
        first += 1;
    }
    let skipped: f32 = h.at.iter().take(first).sum();
    ui.add_space(skipped);
    let mut next = first;
    for (r, hi) in h.at.iter_mut().enumerate().skip(first) {
        if ui.cursor().min.y - top > viewport.max.y {
            break;
        }
        let before = ui.cursor().min.y;
        ui.push_id(("row", r), |ui| row(ui, r));
        *hi = (ui.cursor().min.y - before).max(0.0);
        next = r + 1;
    }
    let rest: f32 = h.at.iter().skip(next).sum();
    ui.add_space(rest);
    ui.data_mut(|d| d.insert_temp(id, h));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn app() -> CadApp {
        CadApp::new(cadaddy_engine::Session::new(), crate::Services::default())
    }

    #[test]
    fn a_reply_is_kept_until_the_drawing_or_the_parameters_change() {
        let mut app = app();
        let ctx = egui::Context::default();
        let made = std::cell::Cell::new(0);
        let ask = |app: &mut CadApp, key: u32| {
            kept(app, &ctx, egui::Id::new("t"), &key, |_| {
                made.set(made.get() + 1);
                key
            })
        };
        assert_eq!(*ask(&mut app, 1), 1);
        assert_eq!(*ask(&mut app, 1), 1);
        assert_eq!(made.get(), 1, "kept while nothing changed");
        assert_eq!(*ask(&mut app, 2), 2);
        assert_eq!(made.get(), 2, "a new key asks again");
        app.run("line", json!({ "points": [[0, 0], [10, 0]] })).unwrap();
        ask(&mut app, 2);
        assert_eq!(made.get(), 3, "a change of the drawing asks again");
        app.session.undo().unwrap();
        ask(&mut app, 2);
        assert_eq!(made.get(), 4, "and so does undoing it");
        ask(&mut app, 2);
        assert_eq!(made.get(), 4);
    }

    #[test]
    fn a_drawing_changed_without_a_new_revision_is_a_new_state() {
        let mut app = app();
        let before = stamp(&app);
        assert_eq!(before, stamp(&app));
        if let Ok(d) = app.session.doc_mut() {
            d.building_spec = Some("{}".to_string()).into();
        }
        assert_ne!(before, stamp(&app), "the drawing was replaced");
    }

    #[test]
    fn only_the_rows_on_screen_are_laid_out() {
        let ctx = egui::Context::default();
        let mut laid = Vec::new();
        for _ in 0..3 {
            laid.clear();
            let raw =
                egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0))), ..Default::default() };
            let mut out = ctx.run_ui(raw, |ui| {
                egui::ScrollArea::vertical().max_height(200.0).show_viewport(ui, |ui, viewport| {
                    rows_in_view(ui, viewport, egui::Id::new("rows"), &1u64, 10_000, |ui, r| {
                        laid.push(r);
                        ui.label(format!("row {r}"));
                    });
                });
            });
            out.textures_delta.clear();
        }
        assert_eq!(laid.first(), Some(&0));
        assert!(!laid.is_empty() && laid.len() < 30, "{} rows laid out of 10,000", laid.len());
    }
}
