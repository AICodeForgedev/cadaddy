//! How long the app took to start: marks from the start of `main` to the first frame and to the
//! first frame that answers input promptly (interactive), with the work in between named.
//!
//! - `ui.startup` (control channel) returns them; they are logged once interactive.
//! - Drawings opened at start (on the command line) are read in the background: their marks say
//!   when each was read and added, and when the first frame showing them was drawn.
//! - `CADADDY_STARTUP_EXIT=1` prints them to stderr as JSON and quits once interactive and the
//!   drawings opened at start are on screen (for timing
//!   start-up from a script, cold and warm). Each mark also carries the wall clock in milliseconds
//!   since the Unix epoch, so a script that notes when it launched the app also sees the time
//!   before `main` (loading the executable).

use std::sync::Mutex;

use serde_json::{Value, json};

/// A frame drawn in less than this, after the first, means the window answers input: interactive.
pub const INTERACTIVE_FRAME_MS: f64 = 50.0;

#[derive(Default)]
struct Clock {
    /// Wall clock at the start of `main` (ms since the epoch); 0 until [`begin`].
    start: f64,
    marks: Vec<(String, f64)>,
    first_frame: Option<f64>,
    interactive: Option<f64>,
    frames: u32,
    /// Opens started before start-up was done, and how many of them are still being read.
    opens: u32,
    pending: u32,
    /// The first frame with every one of them on screen.
    shown: Option<f64>,
    done: bool,
}

fn clock() -> &'static Mutex<Clock> {
    static CLOCK: Mutex<Clock> = Mutex::new(Clock {
        start: 0.0,
        marks: Vec::new(),
        first_frame: None,
        interactive: None,
        frames: 0,
        opens: 0,
        pending: 0,
        shown: None,
        done: false,
    });
    &CLOCK
}

/// Start the clock: first thing in `main`.
pub fn begin() {
    if let Ok(mut c) = clock().lock()
        && c.start == 0.0
    {
        c.start = crate::now_ms();
    }
}

/// Note that `what` is done (ms since [`begin`]).
pub fn mark(what: &str) {
    let now = crate::now_ms();
    if let Ok(mut c) = clock().lock()
        && c.start > 0.0
        && !c.done
    {
        let at = now - c.start;
        c.marks.push((what.to_string(), at));
    }
}

/// A drawing started opening (while starting up).
pub fn open_started() {
    if let Ok(mut c) = clock().lock()
        && c.start > 0.0
        && !c.done
    {
        c.opens = c.opens.saturating_add(1);
        c.pending = c.pending.saturating_add(1);
    }
}

/// A drawing started at start-up finished opening (read and added, or failed).
pub fn open_finished(what: &str) {
    mark(what);
    if let Ok(mut c) = clock().lock()
        && !c.done
    {
        c.pending = c.pending.saturating_sub(1);
    }
}

/// A frame ended after `frame_ms` of drawing. Returns `true` once, on the frame that finishes
/// start-up: interactive, and the drawings opened at start on screen.
pub fn frame(frame_ms: f64) -> bool {
    let now = crate::now_ms();
    let Ok(mut c) = clock().lock() else { return false };
    if c.start == 0.0 || c.done {
        return false;
    }
    c.frames = c.frames.saturating_add(1);
    let at = now - c.start;
    if c.first_frame.is_none() {
        c.first_frame = Some(at);
        c.marks.push(("first frame".into(), at));
        return false;
    }
    if c.interactive.is_none() && frame_ms < INTERACTIVE_FRAME_MS {
        c.interactive = Some(at);
        c.marks.push(("interactive".into(), at));
    }
    if c.opens > 0 && c.pending == 0 && c.shown.is_none() {
        c.shown = Some(at);
        c.marks.push(("drawings on screen".into(), at));
    }
    c.done = c.interactive.is_some() && c.pending == 0;
    c.done
}

/// The marks as JSON: `{startEpochMs, firstFrameMs, interactiveMs, frames, marks: [{what, ms}]}`.
pub fn report() -> Value {
    let Ok(c) = clock().lock() else { return Value::Null };
    json!({
        "startEpochMs": c.start,
        "firstFrameMs": c.first_frame,
        "interactiveMs": c.interactive,
        "drawingsShownMs": c.shown,
        "frames": c.frames,
        "marks": c.marks.iter().map(|(w, ms)| json!({"what": w, "ms": (ms * 10.0).round() / 10.0})).collect::<Vec<_>>(),
    })
}

/// Whether the app should quit once interactive (`CADADDY_STARTUP_EXIT=1`).
pub fn exit_when_interactive() -> bool {
    std::env::var_os("CADADDY_STARTUP_EXIT").is_some_and(|v| v != "0")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_first_frame_then_interactive() {
        begin();
        mark("window");
        assert!(!frame(400.0), "the first frame is never interactive");
        open_started();
        assert!(!frame(120.0), "a slow frame isn't");
        assert!(!frame(8.0), "interactive, but the drawing isn't read yet");
        open_finished("read plan.dwg");
        assert!(frame(30.0), "the drawing's first frame");
        assert!(!frame(8.0), "only once");
        mark("after");
        let r = report();
        let whats: Vec<&str> = r["marks"].as_array().unwrap().iter().map(|m| m["what"].as_str().unwrap()).collect();
        assert_eq!(whats, ["window", "first frame", "interactive", "read plan.dwg", "drawings on screen"]);
        assert_eq!(r["frames"], 4);
        assert!(r["interactiveMs"].as_f64().unwrap() >= r["firstFrameMs"].as_f64().unwrap());
    }
}
