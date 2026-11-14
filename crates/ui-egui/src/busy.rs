//! Long engine work off the UI thread (Q6269). A command that can run long
//! ([`cadaddy_engine::CommandSpec::long`]: a job, a stage, a model build, a takeoff) runs on a worker
//! thread that takes the session for the run. The window keeps a read-only copy of the drawings
//! ([`Session::view_copy`]) and keeps painting it, under a card that names the command, the job's
//! stage and progress (its `job.progress` reports), the time taken and a Cancel (Esc). The control
//! channel keeps answering: `ping`, `cmdline.state` (busy, and what runs), `job.status` (the live
//! progress), `control.cancel`, `cmdline.key` Escape, and what only reads the window; anything else
//! is refused as busy until the work ends. When it ends the session comes back and what was to
//! follow (the history echo, the Takeoff window, an agent's reply) runs on the UI thread. A run
//! that ends within [`GRACE`] ends as if it had run on the UI thread, with no card. The web build
//! has no threads: the work runs in place there.

use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError, channel};

use cadaddy_engine::Session;
use egui::RichText;
use serde_json::{Value, json};
use web_time::{Duration, Instant};

use crate::CadApp;
use crate::theme::Tokens;

/// How long the window waits for the work before it shows the card: a run this quick ends as if
/// it had run on the UI thread.
pub const GRACE: Duration = Duration::from_millis(150);

/// The work: what runs on the worker thread with the session.
pub type Work = Box<dyn FnOnce(&mut Session) -> Result<Value, String> + Send>;
/// What follows the work, on the UI thread, with its result.
pub type Then = Box<dyn FnOnce(&mut CadApp, Result<Value, String>)>;

type Finished = (Session, Result<Value, String>);

/// Work running on the worker thread.
pub struct Busy {
    command: String,
    started: Instant,
    rx: Receiver<Finished>,
    then: Option<Then>,
    /// When Cancel was asked for, and whether a job was there to take it.
    cancel: Option<bool>,
}

/// Whether work can run off the UI thread at all (not on the web, which has no threads).
pub fn backgrounds_anything() -> bool {
    cfg!(not(target_arch = "wasm32"))
}

/// Whether `command` (an id or alias) runs off the UI thread when an interface starts it.
pub fn backgrounds(command: &str) -> bool {
    cfg!(not(target_arch = "wasm32")) && cadaddy_engine::cmd::is_long(command)
}

/// The worker thread, waiting for its session and work. `None` when no thread could be started.
fn spawn_worker() -> Option<(Sender<(Session, Work)>, Receiver<Finished>)> {
    let (tx_in, rx_in) = channel::<(Session, Work)>();
    let (tx_out, rx_out) = channel::<Finished>();
    std::thread::Builder::new()
        .name("cadaddy-work".into())
        .spawn(move || {
            let Ok((mut s, work)) = rx_in.recv() else { return };
            // The session always goes back, whatever the work does.
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(&mut s)))
                .unwrap_or_else(|_| Err("the command stopped on an internal error; the drawing is kept".to_string()));
            let _ = tx_out.send((s, r));
        })
        .ok()?;
    Some((tx_in, rx_out))
}

/// Run `work` on a worker thread with the session (see the module); `then` gets its result on
/// the UI thread. Refused (an error to `then`) while other work runs; run in place when there are
/// no threads.
pub fn start(app: &mut CadApp, command: &str, work: Work, then: Then) {
    if let Some(msg) = refusal(app) {
        then(app, Err(msg));
        return;
    }
    let worker = if cfg!(target_arch = "wasm32") { None } else { spawn_worker() };
    let Some((tx, rx)) = worker else {
        let r = work(&mut app.session);
        then(app, r);
        return;
    };
    let copy = app.session.view_copy();
    let session = std::mem::replace(&mut app.session, copy);
    if let Err(std::sync::mpsc::SendError((session, work))) = tx.send((session, work)) {
        // The thread is gone already: the work runs here.
        app.session = session;
        let r = work(&mut app.session);
        then(app, r);
        return;
    }
    // The copy keeps the agent flag of the caller; the caller puts back its own when it returns.
    app.busy = Some(Busy { command: command.to_string(), started: Instant::now(), rx, then: Some(then), cancel: None });
    wait(app, GRACE);
}

/// Wait up to `timeout` for the running work to end (finishing it when it does). True when no
/// work is left running.
pub fn wait(app: &mut CadApp, timeout: Duration) -> bool {
    let Some(b) = &app.busy else { return true };
    match b.rx.recv_timeout(timeout) {
        Ok((s, r)) => finish(app, s, r),
        Err(RecvTimeoutError::Timeout) => return false,
        Err(RecvTimeoutError::Disconnected) => lost(app),
    }
    true
}

/// Each frame: finish work that has ended; while it runs, keep frames coming for the card.
pub fn poll(app: &mut CadApp, ctx: &egui::Context) {
    let Some(b) = &app.busy else { return };
    match b.rx.try_recv() {
        Ok((s, r)) => finish(app, s, r),
        Err(TryRecvError::Disconnected) => lost(app),
        Err(TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(100)),
    }
}

fn finish(app: &mut CadApp, session: Session, r: Result<Value, String>) {
    let Some(mut b) = app.busy.take() else { return };
    // The flag the caller put back on the copy is the one the session has at rest.
    let from_agent = app.session.agent.from_agent;
    app.session = session;
    app.session.agent.from_agent = from_agent;
    if let Some(then) = b.then.take() {
        then(app, r);
    }
}

/// The worker ended without giving the session back (it can't, short of an abort): the copy
/// stays, the drawings as they were when the work started.
fn lost(app: &mut CadApp) {
    let Some(mut b) = app.busy.take() else { return };
    let msg = format!("{} stopped without finishing; the drawings are as they were before it", b.command);
    if let Some(then) = b.then.take() {
        then(app, Err(msg));
    }
}

/// Whether work is running.
pub fn running(app: &CadApp) -> Option<&str> {
    app.busy.as_ref().map(|b| b.command.as_str())
}

/// Why a command can't start now: work is running. `None` when idle.
pub fn refusal(app: &CadApp) -> Option<String> {
    let b = app.busy.as_ref()?;
    Some(format!("CADaddy is busy running {}; wait for it to finish, or cancel it (Esc, or control.cancel)", b.command))
}

/// The running job's live progress (`None` when no job reports, e.g. a takeoff).
fn job_progress() -> Option<Value> {
    cadaddy_engine::cmd::guided::live_progress().filter(|p| p["running"] == json!(true))
}

/// What runs, for the control channel: the command, the time taken, the job's live progress.
pub fn status(app: &CadApp) -> Option<Value> {
    let b = app.busy.as_ref()?;
    let elapsed = u64::try_from(b.started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let mut v = json!({
        "busy": true,
        "running": b.command,
        "elapsed_ms": elapsed,
        "note": "the command runs in the background; the window shows its progress. The full answer comes when it ends.",
    });
    if let Some(p) = job_progress()
        && let Some(o) = v.as_object_mut()
    {
        for k in ["stage", "step", "progress", "line", "cancel_requested"] {
            if let Some(x) = p.get(k) {
                o.insert(if k == "stage" { "current_stage".into() } else { k.into() }, x.clone());
            }
        }
    }
    if let (Some(c), Some(o)) = (b.cancel, v.as_object_mut()) {
        o.insert("cancel".into(), json!(if c { "asked: it stops at its next sheet or stage" } else { "asked: this step can't stop early" }));
    }
    Some(v)
}

/// Ask the running work to stop (a job stops at its next sheet or stage).
pub fn cancel(app: &mut CadApp) {
    let job = job_progress().is_some();
    cadaddy_engine::request_cancel();
    if let Some(b) = &mut app.busy {
        b.cancel = Some(job);
    }
}

/// The control requests answered while work runs: `Some(reply)` for one handled here (refused,
/// or answered from the work's state), `None` for one the normal handler may serve (it only reads
/// the window).
pub fn control(app: &mut CadApp, method: &str, params: &Value) -> Option<Value> {
    let st = status(app)?;
    match method {
        "ping" | "auth" | "control.methods" | "control.cancel" | "cmdline.state" | "ui.inspect" | "ui.elements" | "ui.screenshot"
        | "ui.menu.list" | "engine.commands" => None,
        "cmdline.key" if params.get("key").and_then(Value::as_str).is_some_and(|k| matches!(k.to_ascii_lowercase().as_str(), "escape" | "esc")) => {
            cancel(app);
            Some(json!({"ok": true, "result": status(app).unwrap_or(st)}))
        }
        "engine.execute" | "command"
            if params.get("command").or(params.get("id")).and_then(Value::as_str).is_some_and(|c| c.eq_ignore_ascii_case("job.status")) =>
        {
            Some(json!({"ok": true, "result": st}))
        }
        _ => {
            let what = st["current_stage"].as_str().map_or_else(String::new, |s| {
                let pct = st["progress"].as_f64().map_or_else(String::new, |p| format!(", {:.0}%", (p * 100.0).clamp(0.0, 100.0)));
                format!(" ({s}{pct})")
            });
            Some(json!({
                "ok": false,
                "error": format!("busy: CADaddy is running {}{what}; try again when it finishes, or cancel it with control.cancel. ping, cmdline.state and job.status answer meanwhile", st["running"].as_str().unwrap_or("")),
                "state": "not-run",
                "busy": st,
            }))
        }
    }
}

/// `2:05` for 125 s, `1:02:03` past an hour.
fn clock(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

/// The card over the window while work runs: the command, the job's stage and progress, the time
/// taken, and Cancel. It holds the window's input (the drawings are read-only meanwhile).
pub fn show(app: &mut CadApp, ctx: &egui::Context) {
    let Some(b) = &app.busy else { return };
    let t = Tokens::get();
    let progress = job_progress();
    let command = b.command.clone();
    let elapsed = clock(b.started.elapsed());
    let asked = b.cancel;
    let mut cancel_now = false;
    egui::Modal::new(egui::Id::new("cc_busy")).show(ctx, |ui| {
        ui.set_width(440.0);
        ui.horizontal(|ui| {
            ui.add(egui::Spinner::new().color(t.accent));
            ui.label(RichText::new(crate::tf!("Running {command}", command = command)).strong().color(t.text));
        });
        if let Some(p) = &progress {
            if let Some(stage) = p["stage"].as_str() {
                ui.label(crate::tf!("Stage: {stage}", stage = stage));
            }
            if let Some(f) = p["progress"].as_f64().filter(|f| f.is_finite()) {
                let f = f.clamp(0.0, 1.0) as f32;
                ui.add(egui::ProgressBar::new(f).show_percentage());
            }
            if let Some(line) = p["line"].as_str().filter(|l| !l.is_empty()) {
                ui.label(RichText::new(line).small().color(t.text_dim));
            }
        }
        ui.label(crate::tf!("Elapsed: {time}", time = elapsed));
        ui.label(RichText::new(crate::tl!("The drawings are read-only until it finishes.")).small().color(t.text_dim));
        match asked {
            Some(true) => {
                ui.label(crate::tl!("Stopping at the next sheet or stage…"));
            }
            Some(false) => {
                ui.label(crate::tl!("This step can't stop early; it ends on its own."));
            }
            None => {}
        }
        ui.add_space(4.0);
        let label = crate::tl!("Cancel");
        let r = ui.add_enabled(asked.is_none(), egui::Button::new(label));
        crate::control::record_widget(ui, &r, label, "button", None);
        if r.clicked() {
            cancel_now = true;
        }
    });
    if asked.is_none() && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        cancel_now = true;
    }
    if cancel_now {
        cancel(app);
    }
    ctx.request_repaint_after(Duration::from_millis(100));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_reads_minutes_and_hours() {
        assert_eq!(clock(Duration::from_secs(0)), "0:00");
        assert_eq!(clock(Duration::from_secs(125)), "2:05");
        assert_eq!(clock(Duration::from_secs(3723)), "1:02:03");
    }

    #[test]
    fn job_and_takeoff_commands_run_off_the_ui_thread() {
        if cfg!(target_arch = "wasm32") {
            return;
        }
        for id in ["job.start", "job.next", "job.answer", "job.set_mode", "automation.takeoff", "takeoff", "model.build", "bim.from_pdf", "validate"]
        {
            assert!(backgrounds(id), "{id}");
        }
        for id in ["line", "job.status", "job.report", "zoom", "nosuch"] {
            assert!(!backgrounds(id), "{id}");
        }
    }
}
