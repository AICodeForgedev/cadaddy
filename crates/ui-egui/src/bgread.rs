//! Reads of the drawing worked out off the UI thread (Q6574). A dialog or palette that shows what
//! a query of the whole model answers (Quick Select's object types and properties, the Model
//! Tree, the job's status and worklist) asks for it here: the query runs on a worker thread, on a
//! read-only copy of the session ([`Session::view_copy`]: the same drawings, shared), and the
//! dialog draws at once, saying it is reading, until the answer comes. An answer that comes within
//! [`GRACE`] is there in the same frame, as if read in place, so a small drawing never shows the
//! reading line. One read at a time per asker: asked again for something else while one runs, it
//! waits for that one, drops it and starts the new one. The web build has no threads: the query
//! runs in place there.
//!
//! Only for queries: what the command changes happens to the copy and is dropped.

use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, TryRecvError, channel};
use std::sync::{Arc, Mutex};

use cadaddy_engine::Session;
use serde_json::Value;
use web_time::Duration;

use crate::CadApp;

/// What a dialog shows while its read runs (translated where it is shown).
pub const READING: &str = "Reading the drawing…";

/// How long the frame that asks waits for the answer before it draws without it.
pub const GRACE: Duration = Duration::from_millis(40);

/// Reads running now (the tests wait for them).
static RUNNING: AtomicUsize = AtomicUsize::new(0);

type Answer = Result<Value, String>;

/// A read running: what it was asked for, and where its answer comes.
#[derive(Clone)]
struct Pending {
    asked: u64,
    rx: Arc<Mutex<Receiver<Answer>>>,
}

#[derive(Clone, Default)]
struct Slot {
    pending: Option<Pending>,
}

/// The answer to `command` with `params` on the drawing as it is (`key`: whatever the asker's
/// answer depends on besides the command and its parameters, the drawing first), or `None` while
/// it is being worked out (a frame follows when it comes). The asker keeps the answer: asked again
/// once answered, the query runs again.
pub fn read(app: &mut CadApp, ctx: &egui::Context, id: egui::Id, key: impl Hash, command: &str, params: &Value) -> Option<Answer> {
    let asked = {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        key.hash(&mut h);
        command.hash(&mut h);
        params.to_string().hash(&mut h);
        h.finish()
    };
    let id = id.with("bgread");
    let mut slot: Slot = crate::kept::take(ctx, id).unwrap_or_default();
    if let Some(p) = slot.pending.take() {
        let got = p.rx.lock().map(|rx| rx.try_recv()).unwrap_or(Err(TryRecvError::Disconnected));
        match got {
            Ok(answer) if p.asked == asked => return Some(answer),
            // An answer to something asked before: dropped, the new read starts.
            Ok(_) | Err(TryRecvError::Disconnected) => {}
            Err(TryRecvError::Empty) => {
                slot.pending = Some(p);
                crate::kept::put(ctx, id, slot);
                ctx.request_repaint_after(Duration::from_millis(50));
                return None;
            }
        }
    }
    let Some(rx) = start(app, ctx, command, params) else {
        // No thread: read in place.
        return Some(app.session.execute(command, params).map_err(|e| e.to_string()));
    };
    let first = rx.recv_timeout(GRACE);
    match first {
        Ok(answer) => Some(answer),
        Err(RecvTimeoutError::Disconnected) => Some(app.session.execute(command, params).map_err(|e| e.to_string())),
        Err(RecvTimeoutError::Timeout) => {
            slot.pending = Some(Pending { asked, rx: Arc::new(Mutex::new(rx)) });
            crate::kept::put(ctx, id, slot);
            ctx.request_repaint_after(Duration::from_millis(50));
            None
        }
    }
}

/// Whether a read is still running (for tests and benchmarks that wait for the window to settle).
pub fn running() -> bool {
    RUNNING.load(Ordering::Relaxed) > 0
}

/// Wait up to `timeout` for the reads running to end (their answers are taken by the next frame).
/// True when none is left running.
pub fn wait(timeout: Duration) -> bool {
    let until = web_time::Instant::now() + timeout;
    while running() {
        if cfg!(target_arch = "wasm32") || web_time::Instant::now() >= until {
            return false;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    true
}

/// The query on a worker thread, on a copy of the session; `None` when no thread can run it.
fn start(app: &CadApp, ctx: &egui::Context, command: &str, params: &Value) -> Option<Receiver<Answer>> {
    if cfg!(target_arch = "wasm32") {
        return None;
    }
    let mut copy: Session = app.session.view_copy();
    let (tx, rx) = channel::<Answer>();
    let (command, params, ctx) = (command.to_string(), params.clone(), ctx.clone());
    RUNNING.fetch_add(1, Ordering::Relaxed);
    let spawned = std::thread::Builder::new().name("cadaddy-read".into()).spawn(move || {
        let answer = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| copy.execute(&command, &params).map_err(|e| e.to_string())))
            .unwrap_or_else(|_| Err(format!("{command} stopped on an internal error")));
        let _ = tx.send(answer);
        RUNNING.fetch_sub(1, Ordering::Relaxed);
        ctx.request_repaint();
    });
    if spawned.is_err() {
        RUNNING.fetch_sub(1, Ordering::Relaxed);
        return None;
    }
    Some(rx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn app_with_lines(n: usize) -> CadApp {
        let mut s = Session::new();
        for i in 0..n {
            s.execute("line", &json!({ "points": [[i, 0], [i, 1]] })).ok();
        }
        CadApp::new(s, crate::Services::default())
    }

    #[test]
    fn a_quick_read_is_answered_in_the_frame_that_asks() {
        let mut app = app_with_lines(3);
        let ctx = egui::Context::default();
        let r = read(&mut app, &ctx, egui::Id::new("t1"), 1u8, "qselect.info", &json!({}));
        assert_eq!(r.and_then(Result::ok).and_then(|v| v["count"].as_u64()), Some(3));
    }

    #[test]
    fn the_answer_is_of_the_query_asked_last() {
        let mut app = app_with_lines(2);
        let ctx = egui::Context::default();
        let id = egui::Id::new("t2");
        let mut last = None;
        for _ in 0..200 {
            if let Some(r) = read(&mut app, &ctx, id, 2u8, "qselect.info", &json!({ "type": "Line" })) {
                last = r.ok();
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(last.is_some_and(|v| v["properties"].as_array().is_some_and(|a| a.iter().any(|p| p == "length"))));
    }
}
