//! The picture of the open sheet the canvas shows zoomed out (Q6417).
//!
//! With a print set's sheet open (`cadaddy_engine::sheetnav`) and the view zoomed out so that
//! the sheet's picture has at least one pixel for every screen pixel, the canvas draws that
//! picture instead of the sheet's linework: one textured quad, whatever the sheet holds. Zooming
//! in past the picture's resolution swaps back to the linework (the display list, built for the
//! visible area at the zoom's detail level), and the list or the picture are drawn while the
//! other is being made, so the screen never blanks.
//!
//! The picture is made off the UI thread (its own worker, cancelled when it is no longer wanted:
//! another sheet, an edit, the view zoomed in) from the drawing as the screen shows it, at the
//! detail level of its own pixel size. An edit makes it stale at once: the linework is drawn
//! until the picture of the edited sheet is ready. A few recent sheets' pictures are kept, so
//! stepping back and forth through sheets shows them at once. The web build has no threads and
//! draws the linework.
//!
//! Drawing only: picking, selection and snaps read the drawing as before (the display list stays
//! built for highlights), and plots never see the picture.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};

use cadaddy_color::Rgb;
use cadaddy_doc::{Drawing, Space};
use cadaddy_geom::{Bounds2, Vec2};
use cadaddy_render::detail::{Detail, PlanRules};
use egui::{Color32, Pos2, Rect};

/// Pictures kept (the open sheet's and the last few others').
const MAX_KEPT: usize = 4;

/// What a picture is of: the drawing (by its id and revision), the sheet, its rectangle, the
/// background and its size in pixels.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Key {
    pub uid: u64,
    pub revision: u64,
    pub sheet: usize,
    rect: [u64; 4],
    bg: Rgb,
    size: [u32; 2],
}

impl Key {
    fn same_sheet(&self, o: &Key) -> bool {
        self.uid == o.uid && self.sheet == o.sheet && self.rect == o.rect && self.bg == o.bg && self.size == o.size
    }
}

/// What the screen wants a picture of now: its key, the sheet's rectangle and a texel's size in
/// world units.
#[derive(Clone, Debug)]
pub(crate) struct Want {
    pub key: Key,
    pub rect: Bounds2,
    pub texel: f64,
}

impl Want {
    /// For sheet `rect` of drawing `uid` at `revision`, `image_px` pixels along its long side.
    pub fn new(uid: u64, revision: u64, sheet: usize, rect: Bounds2, bg: Rgb, image_px: u32) -> Option<Want> {
        let (w, h) = (rect.width(), rect.height());
        if !(w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0) {
            return None;
        }
        let long = f64::from(image_px.max(1));
        let texel = w.max(h) / long;
        let size = [((w / texel).ceil() as u32).clamp(1, image_px.max(1)), ((h / texel).ceil() as u32).clamp(1, image_px.max(1))];
        let rect_bits = [rect.min.x.to_bits(), rect.min.y.to_bits(), rect.max.x.to_bits(), rect.max.y.to_bits()];
        Some(Want { key: Key { uid, revision, sheet, rect: rect_bits, bg, size }, rect, texel })
    }

    /// Whether the picture is fine enough for a screen pixel of `px` world units.
    pub fn covers(&self, px: f64) -> bool {
        px.is_finite() && px >= self.texel
    }
}

struct Picture {
    key: Key,
    rect: Bounds2,
    texture: egui::TextureHandle,
    bytes: usize,
}

/// A picture being made: what it is of and where its pixels arrive. Dropping it cancels it.
struct Job {
    want: Want,
    rx: Receiver<Option<Vec<u8>>>,
    cancel: Arc<AtomicBool>,
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// The canvas's sheet pictures.
#[derive(Default)]
pub struct SheetImages {
    made: Vec<Picture>,
    job: Option<Job>,
    /// The last frame drew a picture instead of the list.
    pub shown: bool,
    /// The detail level the picture drawn was made at (the snaps follow it while it is shown).
    pub shown_detail: Option<Detail>,
    /// Pictures made (for measuring).
    pub builds: u64,
    /// Milliseconds the last one took on its worker.
    pub build_ms: f64,
}

impl std::fmt::Debug for SheetImages {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SheetImages").field("kept", &self.made.len()).field("shown", &self.shown).field("builds", &self.builds).finish()
    }
}

impl SheetImages {
    /// Whether a picture for `want` is ready (made for this revision).
    pub(crate) fn ready(&self, want: &Want) -> bool {
        self.made.iter().any(|p| p.key == want.key)
    }

    /// Bytes the kept pictures take (on the GPU, or in the CPU canvas's textures).
    pub fn bytes(&self) -> usize {
        self.made.iter().map(|p| p.bytes).sum()
    }

    /// Whether a picture is being made.
    pub fn making(&self) -> bool {
        self.job.is_some()
    }

    /// Drop every picture and the one being made.
    pub fn clear(&mut self) {
        self.made.clear();
        self.job = None;
        self.shown = false;
        self.shown_detail = None;
    }

    /// Take a finished picture in.
    fn poll(&mut self, ctx: &egui::Context) {
        let Some(job) = &self.job else { return };
        let got = match job.rx.try_recv() {
            Ok(got) => got,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => None,
        };
        let Some(job) = self.job.take() else { return };
        let Some(px) = got else { return };
        let [w, h] = job.want.key.size;
        let (w, h) = (w as usize, h as usize);
        if px.len() != w * h * 4 {
            return;
        }
        let img = egui::ColorImage::from_rgba_unmultiplied([w, h], &px);
        let texture = ctx.load_texture(format!("sheet-picture-{}-{}", job.want.key.uid, job.want.key.sheet), img, egui::TextureOptions::LINEAR);
        // A newer picture of the same sheet replaces the old one; the oldest go past the limit.
        self.made.retain(|p| !p.key.same_sheet(&job.want.key));
        self.made.push(Picture { key: job.want.key.clone(), rect: job.want.rect, texture, bytes: px.len() });
        while self.made.len() > MAX_KEPT {
            self.made.remove(0);
        }
        self.builds = self.builds.saturating_add(1);
    }

    /// Start making the picture `want` from `doc` (the drawing as the screen shows it) unless it
    /// is ready or being made; any other picture being made is cancelled.
    fn request(&mut self, want: &Want, doc: Arc<Drawing>, rules: Arc<PlanRules>, ctx: &egui::Context) {
        if self.ready(want) || self.job.as_ref().is_some_and(|j| j.want.key == want.key) {
            return;
        }
        self.job = spawn(want.clone(), doc, rules, ctx);
    }

    /// Draw the picture for `want` when the view (a screen pixel of `px` world units) is zoomed
    /// out enough for it and it is ready; ask for it when it isn't. True when it was drawn: the
    /// caller then leaves the linework out.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn draw(
        &mut self,
        want: Option<&Want>,
        doc: impl FnOnce() -> Option<Arc<Drawing>>,
        rules: Arc<PlanRules>,
        px: f64,
        p: &egui::Painter,
        xf: &super::Xf,
        ctx: &egui::Context,
    ) -> bool {
        self.poll(ctx);
        self.shown = false;
        self.shown_detail = None;
        let Some(want) = want.filter(|w| w.covers(px)) else {
            // Zoomed in (or no sheet): the linework; a picture still being made is not needed.
            self.job = None;
            return false;
        };
        if !self.ready(want) {
            if let Some(d) = doc() {
                self.request(want, d, rules, ctx);
            }
            return false;
        }
        self.shown_detail = Detail::new(want.texel, rules);
        let Some(pic) = self.made.iter().find(|p| p.key == want.key) else { return false };
        let a = xf.to_screen(Vec2::new(pic.rect.min.x, pic.rect.max.y));
        let b = xf.to_screen(Vec2::new(pic.rect.max.x, pic.rect.min.y));
        let r = Rect::from_two_pos(a, b);
        p.image(pic.texture.id(), r, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
        self.shown = true;
        true
    }
}

/// Make the picture's pixels: the sheet's objects built at the detail level of a texel, drawn
/// with the CPU rasteriser. `None`: cancelled, or nothing could be drawn. (The web build makes
/// none: it has no worker threads.)
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn make(want: &Want, doc: &Drawing, rules: Arc<PlanRules>, cancel: &Arc<AtomicBool>) -> Option<Vec<u8>> {
    use cadaddy_render::raster::{self, RasterOptions};
    let px = want.texel;
    let opts = cadaddy_render::Options {
        tolerance: px * 0.5,
        min_dash: px * 2.0,
        view: Some(want.rect),
        cancel: Some(cancel.clone()),
        detail: Detail::new(px, rules),
        ..Default::default()
    };
    let list = cadaddy_render::build(doc, &Space::Model, &opts);
    if opts.cancelled() {
        return None;
    }
    let [w, h] = want.key.size;
    let view = raster::View { center: want.rect.center(), scale: 1.0 / px, width: w, height: h };
    let pm = raster::render(&list, &view, &RasterOptions { background: want.key.bg, ..RasterOptions::default() })?;
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    // Opaque background: premultiplied and straight alpha are the same.
    Some(pm.data().to_vec())
}

#[cfg(not(target_arch = "wasm32"))]
fn spawn(want: Want, doc: Arc<Drawing>, rules: Arc<PlanRules>, ctx: &egui::Context) -> Option<Job> {
    let (tx, rx) = std::sync::mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let (c, w, ctx) = (cancel.clone(), want.clone(), ctx.clone());
    let spawned = std::thread::Builder::new().name("cadaddy-sheet-picture".into()).spawn(move || {
        // A panic drops `tx` unsent: the canvas sees the job end with nothing and draws the
        // linework.
        let t0 = crate::now_ms();
        let got = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| make(&w, &doc, rules, &c))).ok().flatten();
        if got.is_some() {
            log::debug!("canvas: sheet picture {}x{} in {:.0} ms", w.key.size[0], w.key.size[1], crate::now_ms() - t0);
        }
        let _ = tx.send(got);
        ctx.request_repaint();
    });
    match spawned {
        Ok(_) => Some(Job { want, rx, cancel }),
        Err(e) => {
            log::warn!("canvas: could not start a sheet picture thread ({e}); drawing the linework");
            None
        }
    }
}

/// The web build has no threads: it draws the linework.
#[cfg(target_arch = "wasm32")]
fn spawn(_want: Want, _doc: Arc<Drawing>, _rules: Arc<PlanRules>, _ctx: &egui::Context) -> Option<Job> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_covers_views_whose_pixels_are_no_finer_than_its_own() {
        let rect = Bounds2::new(Vec2::new(0.0, 0.0), Vec2::new(2000.0, 1000.0));
        let Some(w) = Want::new(1, 1, 0, rect, Rgb(0, 0, 0), 1000) else { panic!("want") };
        assert_eq!(w.key.size, [1000, 500]);
        assert!((w.texel - 2.0).abs() < 1e-12);
        assert!(w.covers(2.0) && w.covers(10.0));
        assert!(!w.covers(1.9) && !w.covers(f64::NAN));
        assert!(Want::new(1, 1, 0, Bounds2::EMPTY, Rgb(0, 0, 0), 1000).is_none());
    }

    #[test]
    fn the_picture_is_the_sheet_drawn() {
        let mut d = Drawing::new_imperial();
        let line = cadaddy_doc::Line { a: cadaddy_geom::Vec3::new(0.0, 50.0, 0.0), b: cadaddy_geom::Vec3::new(200.0, 50.0, 0.0) };
        let Ok(_) = d.add(&Space::Model, cadaddy_doc::Common::default(), cadaddy_doc::EntityKind::Line(line)) else { panic!("add") };
        let rect = Bounds2::new(Vec2::new(0.0, 0.0), Vec2::new(200.0, 100.0));
        let Some(w) = Want::new(1, 1, 0, rect, Rgb(0, 0, 0), 400) else { panic!("want") };
        let cancel = Arc::new(AtomicBool::new(false));
        let Some(px) = make(&w, &d, Arc::new(PlanRules::default()), &cancel) else { panic!("no picture") };
        assert_eq!(px.len(), 400 * 200 * 4);
        // The line across the middle is drawn white on black.
        let row = 100usize;
        let lit = (0..400).filter(|x| px.get((row * 400 + x) * 4).is_some_and(|v| *v > 100)).count();
        assert!(lit > 300, "{lit}");
        cancel.store(true, Ordering::Relaxed);
        assert!(make(&w, &d, Arc::new(PlanRules::default()), &cancel).is_none(), "a cancelled picture makes nothing");
    }
}
