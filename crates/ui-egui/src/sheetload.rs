//! A print set's sheets loaded off the UI thread (Q6418, `cadaddy_engine::sheetstore`).
//!
//! In the interface a print set holds the open sheet and a few recent ones; the others are out of
//! memory. Opening one of those (the sheet list, Page Up, Page Down) and starting a command that
//! may read the whole set (typed, from a menu) load what is needed on a worker thread first, under
//! the busy card ([`crate::busy`]), so the window never stops for it. A programmatic call
//! (`app.run`, the control channel) loads on the spot, inside the engine.

use serde_json::{Value, json};

use crate::{CadApp, busy};

/// The sheet `id` (`sheet.open`, `sheet.next`, `sheet.previous`) with `params` opens, by its place
/// in the list.
fn target(app: &CadApp, id: &str, params: &Value) -> Option<usize> {
    let n = app.session.print_sheets().len();
    let at = app.session.active_sheet().map_or(0, |(i, _)| i);
    match id {
        "sheet.next" => Some((at + 1).min(n.checked_sub(1)?)),
        "sheet.previous" => Some(at.saturating_sub(1)),
        _ => match params.get("sheet") {
            Some(Value::Number(k)) => k.as_u64().and_then(|k| usize::try_from(k).ok()).and_then(|k| k.checked_sub(1)),
            Some(Value::String(num)) => app.session.print_sheets().iter().position(|s| s.number.eq_ignore_ascii_case(num.trim())),
            _ => None,
        },
    }
}

/// Run sheet command `id`: off the UI thread when the sheet it opens is out of memory.
pub fn run_sheet(app: &mut CadApp, id: &str, params: Value) {
    let parked = target(app, id, &params).is_some_and(|i| !app.session.parked_sheet_of(Some(i)).is_empty());
    if !parked || !busy::backgrounds_anything() {
        match app.run(id, params) {
            Ok(v) => {
                if let Some(m) = v.get("message").and_then(Value::as_str) {
                    app.session.echo(m.to_string());
                }
            }
            Err(e) => app.session.echo(e),
        }
        return;
    }
    let command = id.to_string();
    busy::start(
        app,
        id,
        Box::new(move |s| s.execute(&command, &params).map_err(|e| e.user_message())),
        Box::new(|app, r| match r {
            Ok(v) => {
                if let Some(m) = v.get("message").and_then(Value::as_str) {
                    app.session.echo(m.to_string());
                }
            }
            Err(e) => app.session.echo(e),
        }),
    );
}

/// Before the interface starts command `name` (typed, from a menu): when it may read the whole
/// set and sheets are out of memory, load them on a worker thread and run `then` after. True
/// when that happened (the caller stops; `then` goes on).
pub fn whole_set_first(app: &mut CadApp, name: &str, then: impl FnOnce(&mut CadApp) + 'static) -> bool {
    if !app.session.needs_whole_set(name) || !busy::backgrounds_anything() || app.busy.is_some() {
        return false;
    }
    busy::start(
        app,
        name,
        Box::new(|s| s.load_all_sheets().map(|n| json!({ "loaded": n })).map_err(|e| e.user_message())),
        Box::new(move |app, r| match r {
            Ok(_) => then(app),
            Err(e) => app.session.echo(e),
        }),
    );
    true
}

/// Each frame: a print set with no sheet open opens on its first one, fitted to the drawing area.
/// Its sheet list waits on the rail: opening a file opens no palette by itself.
/// With sheets on demand, putting the other sheets out of memory runs on a worker thread when
/// most of the set is still in memory (a job just finished, a file saved before sheet sections).
pub fn open_first_sheet(app: &mut CadApp) {
    let s = &app.session;
    let wants = s.sheets_on_demand && s.active_sheet().is_none() && !s.print_sheets().is_empty();
    let heavy = wants && s.doc().is_ok_and(|d| d.model.len() > PARK_ON_UI_THREAD);
    if !heavy || !busy::backgrounds_anything() {
        if app.session.open_first_sheet() {
            app.canvas.zoom_pending = true;
        }
        return;
    }
    if app.busy.is_some() {
        return;
    }
    busy::start(
        app,
        "sheet.open",
        Box::new(|s| Ok(json!({ "opened": s.open_first_sheet() }))),
        Box::new(|app, r| {
            if r.is_ok_and(|v| v["opened"] == json!(true)) {
                app.canvas.zoom_pending = true;
            }
        }),
    );
}

/// Each frame: after a command that read the whole set loaded every sheet, put them out of
/// memory again once the window is idle (no command running or prompting, no work in the
/// background): the open sheet and the recent ones stay. Off the UI thread when there is much
/// to put out.
pub fn park_when_idle(app: &mut CadApp) {
    // Not while something on screen reads the whole set: the set would be loaded again, on the
    // UI thread, for as long as it is open. That is a frame after a command that reads the set
    // ran (a palette that counts or lists across the sheets every frame), and for as long as the
    // dialog that was open then stays open (one that reads again whenever the drawing changes:
    // putting the sheets out changes it).
    let now = crate::now_ms();
    let asks = app.session.whole_set_asks;
    if ASKS_SEEN.with(|c| c.replace(asks)) != asks {
        HELD_FOR.with(|h| h.replace(app.ui.dialog.clone()));
        // Read again soon after the sheets went out: whatever reads it answers to the drawing
        // changing or to work in the background ending, and putting the sheets out is both. The
        // set then stays in memory, twice as long each time it happens.
        let (parked, wait, _) = BACKOFF.with(std::cell::Cell::get);
        if now - parked < READ_AGAIN_MS {
            let wait = (wait * 2.0).clamp(HOLD_MS, HOLD_MAX_MS);
            BACKOFF.with(|c| c.set((parked, wait, now + wait)));
        } else {
            BACKOFF.with(|c| c.set((parked, 0.0, 0.0)));
        }
        return;
    }
    if app.ui.dialog.is_some() && HELD_FOR.with(|h| *h.borrow() == app.ui.dialog) {
        return;
    }
    HELD_FOR.with(|h| h.replace(None));
    let (_, wait, until) = BACKOFF.with(std::cell::Cell::get);
    if now < until {
        return;
    }
    // While the sheets are going out (in the background) and as they do, the time is kept.
    if app.session.whole_set_in_memory() || app.busy.is_some() {
        BACKOFF.with(|c| c.set((now, wait, until)));
    }
    let s = &app.session;
    if !s.whole_set_in_memory() || s.running.is_some() || s.prompting() || app.busy.is_some() {
        return;
    }
    let heavy = s.doc().is_ok_and(|d| d.model.len() > PARK_ON_UI_THREAD);
    if !heavy || !busy::backgrounds_anything() {
        if let Err(e) = app.session.park_idle_sheets() {
            app.session.echo(e.user_message());
        }
        return;
    }
    busy::start(
        app,
        "sheet.open",
        Box::new(|s| s.park_idle_sheets().map(|done| json!({ "parked": done })).map_err(|e| e.user_message())),
        Box::new(|app, r| {
            if let Err(e) = r {
                app.session.echo(e);
            }
        }),
    );
}

/// A read of the whole set this soon after its sheets went out of memory is taken for an answer
/// to their going out, milliseconds.
const READ_AGAIN_MS: f64 = 15_000.0;
/// How long the set then stays in memory the first time, and at most, milliseconds.
const HOLD_MS: f64 = 20_000.0;
const HOLD_MAX_MS: f64 = 600_000.0;

thread_local! {
    /// When the sheets last went out of memory, how long the set is held after a read that
    /// followed it, and until when it is held now ([`crate::now_ms`] times).
    static BACKOFF: std::cell::Cell<(f64, f64, f64)> = const { std::cell::Cell::new((f64::NEG_INFINITY, 0.0, 0.0)) };
    /// [`cadaddy_engine::Session::whole_set_asks`] as the last frame's [`park_when_idle`] saw it.
    static ASKS_SEEN: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// The dialog that was open when a command last read the whole set: the set stays in memory
    /// while it is.
    static HELD_FOR: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// Forget what holds the whole set in memory ([`park_when_idle`]): the next idle frame puts the
/// sheets out. For a window whose reads so far say nothing about what comes next (a test that
/// opens one dialog after another within milliseconds).
pub fn forget_holds() {
    HELD_FOR.with(|h| h.replace(None));
    BACKOFF.with(|c| c.set((f64::NEG_INFINITY, 0.0, 0.0)));
}

/// Objects in memory above which putting a print set's other sheets out of memory runs off the
/// UI thread.
const PARK_ON_UI_THREAD: usize = 200_000;
