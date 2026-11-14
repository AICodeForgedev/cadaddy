//! Where a frame's CPU time goes (Q6482): each part of the window's logic and drawing timed, so a
//! frame that costs more with a large model open says which part grows. `ui.inspect` reports it
//! as `perf.sections` and the debug overlay (`--debug`) lists the costliest parts.
//!
//! A part is timed with [`time`] (or [`add`] for a time measured already); [`end_frame`] closes
//! the frame. A part timed twice in a frame counts the sum. Parts nest: the docked palettes
//! (`panel:<id>`) and the drawing (`panel:canvas`) are inside `docking`. What egui does after the
//! app's code (tessellation, painting) is not in here: `perf.frameMs` is the app's own part.

use std::cell::RefCell;

use serde_json::{Map, Value, json};

/// How much one frame moves the running average (about the last 20 frames).
const SMOOTHING: f64 = 0.1;
/// Parts kept: a frame names a few dozen.
const MAX_PARTS: usize = 128;

#[derive(Default)]
struct Frames {
    /// The frame being timed.
    now: Vec<(&'static str, f64)>,
    /// The last frame finished.
    last: Vec<(&'static str, f64)>,
    /// The running average per part.
    avg: Vec<(&'static str, f64)>,
    frames: u64,
}

thread_local! {
    static FRAMES: RefCell<Frames> = RefCell::new(Frames::default());
}

fn bump(list: &mut Vec<(&'static str, f64)>, name: &'static str, ms: f64) {
    if let Some(e) = list.iter_mut().find(|(n, _)| *n == name) {
        e.1 += ms;
    } else if list.len() < MAX_PARTS {
        list.push((name, ms));
    }
}

/// Count `ms` to part `name` of this frame.
pub fn add(name: &'static str, ms: f64) {
    if !ms.is_finite() || ms < 0.0 {
        return;
    }
    FRAMES.with(|f| bump(&mut f.borrow_mut().now, name, ms));
}

/// Run `f` as part `name` of this frame.
pub fn time<R>(name: &'static str, f: impl FnOnce() -> R) -> R {
    let t0 = crate::now_ms();
    let r = f();
    add(name, crate::now_ms() - t0);
    r
}

/// The frame is drawn: its parts become the last frame's and go into the averages.
pub fn end_frame() {
    FRAMES.with(|f| {
        let mut f = f.borrow_mut();
        let now = std::mem::take(&mut f.now);
        let first = f.frames == 0;
        for e in &mut f.avg {
            let ms = now.iter().find(|(n, _)| *n == e.0).map_or(0.0, |x| x.1);
            e.1 += (ms - e.1) * SMOOTHING;
        }
        for (name, ms) in &now {
            if !f.avg.iter().any(|(n, _)| n == name) && f.avg.len() < MAX_PARTS {
                // A part new this frame starts at its own time (the first frame: everything).
                f.avg.push((name, if first { *ms } else { *ms * SMOOTHING }));
            }
        }
        // A part no frame has had for a long while goes.
        f.avg.retain(|(_, ms)| *ms > 1e-6);
        f.last = now;
        f.frames = f.frames.saturating_add(1);
    });
}

fn sorted(list: &[(&'static str, f64)]) -> Vec<(&'static str, f64)> {
    let mut v = list.to_vec();
    v.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(b.0)));
    v
}

fn round3(ms: f64) -> f64 {
    (ms * 1000.0).round() / 1000.0
}

/// `perf.sections`: the last frame's parts and their running averages, in milliseconds, and the
/// ten costliest on average (`costliest`: `[name, ms]`, the costliest first).
pub fn report() -> Value {
    FRAMES.with(|f| {
        let f = f.borrow();
        let obj = |list: &[(&'static str, f64)]| {
            let mut m = Map::new();
            for (n, ms) in sorted(list) {
                m.insert(n.to_string(), json!(round3(ms)));
            }
            Value::Object(m)
        };
        let costliest: Vec<Value> = sorted(&f.avg).into_iter().take(10).map(|(n, ms)| json!([n, round3(ms)])).collect();
        json!({ "lastMs": obj(&f.last), "avgMs": obj(&f.avg), "costliest": costliest, "frames": f.frames })
    })
}

/// The `n` costliest parts on average, as the debug overlay lists them.
pub fn overlay_lines(n: usize) -> Vec<String> {
    FRAMES.with(|f| sorted(&f.borrow().avg).into_iter().take(n).map(|(name, ms)| format!("{name:<22}{ms:>7.2} ms")).collect())
}

/// One part's running average (tests and benchmarks).
pub fn average(name: &str) -> Option<f64> {
    FRAMES.with(|f| f.borrow().avg.iter().find(|(n, _)| *n == name).map(|x| x.1))
}

/// Start again (a test measuring one state after another).
pub fn reset() {
    FRAMES.with(|f| *f.borrow_mut() = Frames::default());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parts_add_up_per_frame_and_average() {
        reset();
        add("a", 2.0);
        add("a", 1.0);
        add("b", 4.0);
        add("bad", f64::NAN);
        end_frame();
        let r = report();
        assert_eq!(r["lastMs"]["a"], json!(3.0), "{r}");
        assert_eq!(r["lastMs"]["b"], json!(4.0), "{r}");
        assert!(r["lastMs"].get("bad").is_none(), "{r}");
        // The costliest first.
        assert_eq!(r["costliest"], json!([["b", 4.0], ["a", 3.0]]), "{r}");
        // A part missing from later frames fades from the average.
        for _ in 0..30 {
            add("a", 3.0);
            end_frame();
        }
        assert!(average("b").is_some_and(|b| b < 0.2), "{:?}", average("b"));
        assert!(average("a").is_some_and(|a| (a - 3.0).abs() < 1e-9));
        assert_eq!(overlay_lines(1).len(), 1);
        assert!(overlay_lines(1)[0].starts_with('a'), "{:?}", overlay_lines(1));
        let n = time("t", || 7);
        assert_eq!(n, 7);
        end_frame();
        assert!(report()["lastMs"].get("t").is_some());
        reset();
        assert_eq!(report()["frames"], json!(0));
    }
}
