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

/// Each frame: a print set with no sheet open opens on its first one (its sheet list beside it).
/// With sheets on demand, putting the other sheets out of memory runs on a worker thread when
/// most of the set is still in memory (a job just finished, a file saved before sheet sections).
pub fn open_first_sheet(app: &mut CadApp) {
    let s = &app.session;
    let wants = s.sheets_on_demand && s.active_sheet().is_none() && !s.print_sheets().is_empty();
    let heavy = wants && s.doc().is_ok_and(|d| d.model.len() > PARK_ON_UI_THREAD);
    if !heavy || !busy::backgrounds_anything() {
        if app.session.open_first_sheet() {
            crate::sheetsets::open_sheet_list(app);
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
                crate::sheetsets::open_sheet_list(app);
            }
        }),
    );
}

/// Each frame: after a command that read the whole set loaded every sheet, put them out of
/// memory again once the window is idle (no command running or prompting, no work in the
/// background): the open sheet and the recent ones stay. Off the UI thread when there is much
/// to put out.
pub fn park_when_idle(app: &mut CadApp) {
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

/// Objects in memory above which putting a print set's other sheets out of memory runs off the
/// UI thread.
const PARK_ON_UI_THREAD: usize = 200_000;
