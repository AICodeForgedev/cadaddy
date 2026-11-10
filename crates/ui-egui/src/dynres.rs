//! Dynamic resolution for the 2D canvas, as games do it: while the view pans or zooms, a drawing
//! that the GPU can't draw at the target frame rate is drawn into a smaller picture and scaled up
//! to the canvas; once the view has been still for the settle time it is drawn once at full
//! resolution. At rest the picture is always the full-resolution one.
//!
//! The scale comes from the GPU's own time for drawing the drawing (timestamp queries around the
//! pass that draws it into its kept frame, see `gpu::PassTimer`), not from the frame interval:
//! a pan's frames arrive at the speed of the mouse, and the app's CPU time per frame doesn't
//! shrink with fewer pixels. The [`Controller`]:
//!
//! - steps down one [`LEVELS`] step when the drawing's GPU time runs over its share of the
//!   frame at the target rate ([`BUDGET_SHARE`]), and back up when the step above is predicted
//!   (time ∝ pixels) to fit well inside it ([`UP_MARGIN`]): the gap between the two is the
//!   hysteresis that keeps it from pumping;
//! - waits for [`SAMPLES`] timings at a level before the next decision (timings arrive a frame
//!   or two late, and the first frame at a new size allocates its picture);
//! - gives a step down back when it bought less than [`MIN_GAIN`] (a GPU limited by vertices,
//!   not pixels) and doesn't try again until the view stops;
//! - never goes below the floor (half resolution each way, the last level);
//! - keeps its level from one pan to the next, so a slow drawing starts the next pan at the
//!   scale it needed.
//!
//! The settings: DYNRESMODE (on/off) and DYNRESFPS (the target), in Options ▸ Display; `ui.gpu
//! {dynamicScale}` or `CADADDY_GPU_LIMITS=dynscale=50` forces a scale while moving (tests, and
//! measuring one). A plot, an export, `render` and the screenshot command draw at full
//! resolution: none of them draws through the moving canvas, and the app asks for full
//! resolution while a screenshot is waiting.

use std::cell::RefCell;

/// The scales a moving view is drawn at, each way (pixels drawn = scale²): full, then about
/// 72 %, 50 %, 36 % and 25 % of the pixels. The last is the floor.
pub const LEVELS: [f32; 5] = [1.0, 0.85, 0.71, 0.6, 0.5];
/// The drawing's share of a frame at the target rate: the rest is the interface, the overlays
/// and showing the picture.
pub const BUDGET_SHARE: f64 = 0.75;
/// A step up is taken when the drawing at the next level is predicted to take under this part of
/// its budget.
pub const UP_MARGIN: f64 = 0.7;
/// Timings at a level before the next decision.
pub const SAMPLES: u32 = 3;
/// A step down that saves less than this part of the drawing's time is given back.
pub const MIN_GAIN: f64 = 0.1;
/// How long the view stays still before it is drawn at full resolution (ms): the canvas's own
/// settle time.
pub const SETTLE_MS: f64 = crate::canvas::VP_SETTLE_MS;

/// Chooses the scale a moving view is drawn at.
#[derive(Clone, Debug)]
pub struct Controller {
    /// DYNRESMODE.
    pub on: bool,
    /// DYNRESFPS: the frame time aimed at (ms).
    pub target_ms: f64,
    /// A scale forced while moving (`ui.gpu {dynamicScale}`), instead of the measured one.
    pub forced: Option<f32>,
    /// Draw at full resolution whatever moves (a screenshot is waiting).
    pub full: bool,
    level: usize,
    /// The drawing's GPU time at this level: a running mean, and the timings it holds.
    mean_ms: f64,
    samples: u32,
    /// The level and time before the last step down, until that step is judged.
    probe: Option<(usize, f64)>,
    /// A step down didn't pay: stay at full resolution until the view stops.
    no_gain: bool,
    /// The level a step up was given back at: no step above it until the view stops.
    ceiling: Option<usize>,
    stepped_up: bool,
    /// The last timing taken (ms) and the scale it was taken at.
    pub last: Option<(f64, f32)>,
    /// Timings taken, for `ui.inspect`.
    pub timed: u64,
}

impl Default for Controller {
    fn default() -> Self {
        Controller {
            on: true,
            target_ms: 1000.0 / 30.0,
            forced: None,
            full: false,
            level: 0,
            mean_ms: 0.0,
            samples: 0,
            probe: None,
            no_gain: false,
            ceiling: None,
            stepped_up: false,
            last: None,
            timed: 0,
        }
    }
}

impl Controller {
    /// The settings: on or off, the target frame rate (clamped to 10–120).
    pub fn configure(&mut self, on: bool, fps: u8, forced: Option<f32>) {
        self.on = on;
        self.target_ms = 1000.0 / f64::from(fps.clamp(10, 120));
        self.forced = forced.filter(|s| s.is_finite()).map(|s| s.clamp(floor(), 1.0));
        if !on {
            self.level = 0;
            self.reset_level_state();
        }
    }

    /// The scale a view moving now is drawn at (1 = full resolution).
    pub fn moving_scale(&self) -> f32 {
        if !self.on || self.full {
            return 1.0;
        }
        if let Some(s) = self.forced {
            return s;
        }
        LEVELS.get(self.level).copied().unwrap_or(1.0)
    }

    /// The scale to draw at: [`Self::moving_scale`] while the view moves, else full.
    pub fn scale(&self, moving: bool) -> f32 {
        if moving { self.moving_scale() } else { 1.0 }
    }

    /// The level now (0 = full resolution).
    pub fn level(&self) -> usize {
        self.level
    }

    /// The drawing's GPU time at the current level (ms), when timed.
    pub fn mean_ms(&self) -> Option<f64> {
        (self.samples > 0).then_some(self.mean_ms)
    }

    fn budget(&self) -> f64 {
        self.target_ms * BUDGET_SHARE
    }

    fn reset_level_state(&mut self) {
        self.mean_ms = 0.0;
        self.samples = 0;
    }

    fn set_level(&mut self, level: usize) {
        self.level = level.min(LEVELS.len() - 1);
        self.reset_level_state();
    }

    /// The view stopped: what one pan learnt about a step that didn't pay is forgotten, the level
    /// is kept.
    pub fn still(&mut self) {
        self.no_gain = false;
        self.ceiling = None;
        self.stepped_up = false;
        if let Some((level, _)) = self.probe.take() {
            // A step down that was never judged is given back.
            self.set_level(level);
        }
    }

    /// A timing of the drawing's pass: `ms` of GPU time, drawn at `scale` while the view moved.
    /// Timings at another scale than the current level's (a late one, a forced scale) are not
    /// decisions.
    pub fn sample(&mut self, ms: f64, scale: f32) {
        if !ms.is_finite() || ms < 0.0 {
            return;
        }
        self.timed += 1;
        self.last = Some((ms, scale));
        if !self.on || self.full || self.forced.is_some() {
            return;
        }
        let Some(&want) = LEVELS.get(self.level) else { return };
        if (scale - want).abs() > 1e-3 {
            return;
        }
        self.samples += 1;
        // A running mean over the level's first timings, then a slow average.
        let k = if self.samples <= SAMPLES { 1.0 / f64::from(self.samples) } else { 0.3 };
        self.mean_ms += (ms - self.mean_ms) * k;
        if self.samples < SAMPLES {
            return;
        }
        let mean = self.mean_ms;
        // Judge the last step down first.
        // (The probe is taken either way: it is judged once.)
        if let Some((before, before_ms)) = self.probe.take()
            && mean > before_ms * (1.0 - MIN_GAIN)
        {
            self.set_level(before);
            self.no_gain = true;
            return;
        }
        let budget = self.budget();
        let last = LEVELS.len() - 1;
        if mean > budget && self.level < last && !self.no_gain {
            if self.stepped_up {
                // The step up didn't fit after all: hold below it until the view stops.
                self.ceiling = Some(self.level + 1);
            }
            self.stepped_up = false;
            self.probe = Some((self.level, mean));
            self.set_level(self.level + 1);
            return;
        }
        if self.level > 0 && self.ceiling.is_none_or(|c| self.level > c) {
            let (now, up) = (f64::from(want), f64::from(LEVELS.get(self.level - 1).copied().unwrap_or(1.0)));
            let predicted = mean * (up * up) / (now * now);
            if predicted < budget * UP_MARGIN {
                self.stepped_up = true;
                self.set_level(self.level - 1);
            }
        }
    }
}

/// The floor: the smallest scale a moving view is drawn at.
pub fn floor() -> f32 {
    LEVELS.last().copied().unwrap_or(1.0)
}

/// The size a `size` picture is drawn at, at `scale` each way (at least one pixel).
pub fn scaled_size(size: [u32; 2], scale: f32) -> [u32; 2] {
    if scale >= 1.0 {
        return size;
    }
    let s = f64::from(scale.max(floor()));
    size.map(|n| ((f64::from(n) * s).ceil() as u32).clamp(1, n.max(1)))
}

thread_local! {
    /// The app's controller (one for every canvas: they share the GPU). The app's frames, the
    /// canvas's paint callbacks and the timings are all on the interface's thread.
    static CONTROLLER: RefCell<Controller> = RefCell::new(Controller::default());
}

/// Run `f` on the app's controller (`None` only while it is already borrowed).
pub fn with<R>(f: impl FnOnce(&mut Controller) -> R) -> Option<R> {
    CONTROLLER.with(|c| c.try_borrow_mut().ok().map(|mut c| f(&mut c)))
}

/// What `ui.inspect`'s `perf.dynamicResolution` reports.
pub fn json() -> serde_json::Value {
    with(|c| {
        serde_json::json!({
            "on": c.on,
            "targetFps": (1000.0 / c.target_ms).round(),
            "scale": c.moving_scale(),
            "shown": crate::gpu::shown_scale(),
            "level": c.level(),
            "forced": c.forced,
            "drawMs": c.mean_ms().map(|m| (m * 100.0).round() / 100.0),
            "last": c.last.map(|(ms, s)| serde_json::json!({"ms": (ms * 100.0).round() / 100.0, "scale": s})),
            "timed": c.timed,
            "timer": crate::gpu::pass_timer_on(),
        })
    })
    .unwrap_or(serde_json::Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(fps: u8) -> Controller {
        let mut c = Controller::default();
        c.configure(true, fps, None);
        c
    }

    /// Feed `n` timings of a drawing whose GPU time at full resolution is `full_ms` and falls
    /// with the pixels drawn by `pixel_part` (the rest is per vertex).
    fn run(c: &mut Controller, n: usize, full_ms: f64, pixel_part: f64) {
        for _ in 0..n {
            let s = f64::from(c.moving_scale());
            let ms = full_ms * ((1.0 - pixel_part) + pixel_part * s * s);
            c.sample(ms, c.moving_scale());
        }
    }

    #[test]
    fn a_gpu_that_holds_the_target_stays_at_full_resolution() {
        let mut c = at(30);
        run(&mut c, 50, 10.0, 1.0);
        assert_eq!(c.moving_scale(), 1.0);
        assert_eq!(c.scale(false), 1.0, "at rest always full");
    }

    #[test]
    fn a_slow_gpu_steps_down_to_the_target_and_holds_it() {
        let mut c = at(30);
        // 40 ms at full resolution, all of it pixels; the budget is 25 ms.
        run(&mut c, 60, 40.0, 1.0);
        let s = c.moving_scale();
        assert!(s < 1.0 && s >= floor(), "{s}");
        let ms = 40.0 * f64::from(s * s);
        assert!(ms <= 25.0, "holds the budget: {ms}");
        // It doesn't pump: the level stays over many more frames.
        let level = c.level();
        for _ in 0..200 {
            run(&mut c, 1, 40.0, 1.0);
            assert_eq!(c.level(), level);
        }
        // While still: full resolution; the next pan starts at the learnt level.
        assert_eq!(c.scale(false), 1.0);
        c.still();
        assert_eq!(c.level(), level);
    }

    #[test]
    fn never_below_the_floor() {
        let mut c = at(60);
        run(&mut c, 200, 500.0, 1.0);
        assert_eq!(c.moving_scale(), floor());
        assert_eq!(floor(), 0.5);
    }

    #[test]
    fn steps_back_up_when_there_is_room() {
        let mut c = at(30);
        run(&mut c, 60, 60.0, 1.0);
        let low = c.level();
        assert!(low > 0);
        // A lighter part of the drawing (or the user zoomed in on less).
        run(&mut c, 60, 8.0, 1.0);
        assert_eq!(c.level(), 0, "back at full resolution");
    }

    #[test]
    fn a_step_that_saves_nothing_is_given_back() {
        let mut c = at(30);
        // Vertex-bound: fewer pixels save 2 % of the time.
        run(&mut c, 40, 40.0, 0.02);
        assert_eq!(c.moving_scale(), 1.0, "no point drawing fewer pixels");
        c.still();
        // Tried again on the next pan, given back again.
        run(&mut c, 40, 40.0, 0.02);
        assert_eq!(c.moving_scale(), 1.0);
    }

    #[test]
    fn off_forced_and_full() {
        let mut c = at(30);
        run(&mut c, 60, 60.0, 1.0);
        assert!(c.moving_scale() < 1.0);
        c.full = true;
        assert_eq!(c.moving_scale(), 1.0, "a screenshot waiting");
        c.full = false;
        c.configure(false, 30, None);
        assert_eq!(c.moving_scale(), 1.0, "off");
        assert_eq!(c.level(), 0);
        c.configure(true, 30, Some(0.25));
        assert_eq!(c.moving_scale(), floor(), "a forced scale is held to the floor");
        c.configure(true, 30, Some(0.7));
        run(&mut c, 30, 500.0, 1.0);
        assert_eq!(c.moving_scale(), 0.7, "forced: the timings decide nothing");
    }

    #[test]
    fn late_timings_of_another_level_decide_nothing() {
        let mut c = at(30);
        for _ in 0..10 {
            c.sample(500.0, 0.5);
        }
        assert_eq!(c.level(), 0);
        for bad in [f64::NAN, -1.0, f64::INFINITY] {
            c.sample(bad, 1.0);
        }
        assert_eq!(c.mean_ms(), None);
    }

    #[test]
    fn picture_sizes() {
        assert_eq!(scaled_size([1920, 1200], 1.0), [1920, 1200]);
        assert_eq!(scaled_size([1920, 1200], 0.5), [960, 600]);
        assert_eq!(scaled_size([1921, 3], 0.5), [961, 2]);
        assert_eq!(scaled_size([1, 1], 0.5), [1, 1]);
        assert_eq!(scaled_size([100, 100], 0.01), [50, 50], "never below the floor");
    }
}
