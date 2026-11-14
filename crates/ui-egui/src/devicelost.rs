//! A lost graphics device (a driver reset, a GPU that stopped responding, or one out of memory)
//! doesn't end the session. Nothing more is drawn with the dead device: egui's renderer panics
//! writing a frame's buffers to it, which closed the app (exit 101) with the drawings open. The
//! host closes the window and makes it again on a new device with this same app
//! ([`CadApp::restart_graphics`]), so the drawings, their undo and their file names stay as they
//! were; if no device can be had, it autosaves ([`CadApp::autosave_before_exit`]) and says why.

use crate::CadApp;
use crate::autosave::GpuNotice;
use crate::gpu::{GpuFault, GpuFaults};

impl CadApp {
    /// The graphics device is lost: nothing more is drawn until the host makes the window again
    /// on a new one ([`CadApp::restart_graphics`]).
    pub fn graphics_lost(&self) -> bool {
        self.gpu_faults.is_lost()
    }

    /// At the start of [`CadApp::logic`] and [`CadApp::ui`]: whether the device is lost, so the
    /// frame draws nothing. The loss is autosaved for once (`gpu_fault_tick`), and anything
    /// painted earlier in the frame is dropped.
    pub(crate) fn lost_frame(&mut self, ctx: &egui::Context) -> bool {
        if !self.graphics_lost() {
            return false;
        }
        self.gpu_fault_tick(ctx);
        forget_frame(ctx);
        true
    }

    /// The window was made again on a new device (`rs`; `stencil` as for [`CadApp::set_wgpu`])
    /// after the old one was lost, or after its drawing panicked (`why`, the panic's message).
    /// Everything made on the old device or with the old window's textures is let go, the canvas
    /// and the 3D views draw on the new device, and the notice says once what happened.
    pub fn restart_graphics(&mut self, rs: &egui_wgpu::RenderState, stencil: Option<egui_wgpu::wgpu::TextureFormat>, why: Option<&str>) {
        // What the old device reported that no frame took in yet (a loss noticed mid-frame).
        let reported = self.gpu_faults.take();
        let before = self.gpu_notice.take();
        let mut fault =
            reported.or_else(|| before.as_ref().map(|n| n.fault.clone())).unwrap_or(GpuFault { errors: 0, lost: true, message: String::new() });
        fault.lost = true;
        if let Some(why) = why.filter(|w| !w.is_empty()) {
            // A panic: kept for Help ▸ Report a Problem, like one in a command.
            cadaddy_engine::crash_report::record(&self.session, "window", why);
            fault.message = if fault.message.is_empty() { why.to_string() } else { format!("{}\n{why}", fault.message) };
        }
        let autosaved = match &before {
            Some(n) => n.autosaved,
            None => self.autosave_now(),
        };
        // The old device's handlers keep the old registry: the new device starts with none.
        self.gpu_faults = std::sync::Arc::new(GpuFaults::default());
        self.canvas = crate::canvas::CanvasState::default();
        if let Some(w) = self.walk.as_mut() {
            w.forget_graphics();
        }
        if let Some(q) = self.quickview.as_mut() {
            q.forget_textures();
        }
        self.set_wgpu(rs, stencil);
        log::warn!("graphics restarted on {}", rs.adapter.get_info().name);
        self.gpu_notice = Some(GpuNotice { fault, autosaved, restarted: true });
    }

    /// Autosave every drawing with unsaved changes and wait until it is written: the app is about
    /// to close without a window. Returns how many drawings were written.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn autosave_before_exit(&mut self) -> usize {
        let Some(saver) = &mut self.autosave else { return 0 };
        let session = &self.session;
        // The save is the point of being here: a panic inside it must not end the app first.
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| saver.emergency_save(session))).unwrap_or(0)
    }

    /// Start autosaving every drawing with unsaved changes now (not SAVETIME minutes from now).
    fn autosave_now(&mut self) -> usize {
        #[cfg(not(target_arch = "wasm32"))]
        let saved = self.autosave.as_mut().map_or(0, |a| a.save_now(&self.session));
        #[cfg(target_arch = "wasm32")]
        let saved = 0;
        saved
    }
}

/// Lose the graphics device on request (`ui.gpu {"lose": true}`) as a driver reset or a GPU out of
/// memory would: the device becomes invalid and its lost callback is told, so what follows is what
/// follows a real loss. `{"lost": false}` without a device (the CPU draws).
pub(crate) fn lose_device(app: &mut CadApp) -> serde_json::Value {
    let Some(rs) = &app.wgpu_rs else { return serde_json::json!({ "lost": false }) };
    rs.device.destroy();
    app.gpu_faults.device_lost(egui_wgpu::wgpu::DeviceLostReason::Unknown, "lost on request (ui.gpu lose)".into());
    serde_json::json!({ "lost": true })
}

/// Drop what this frame painted so far: it would be drawn with the dead device.
fn forget_frame(ctx: &egui::Context) {
    let layers: Vec<egui::LayerId> = ctx.memory(|m| m.layer_ids().collect());
    ctx.graphics_mut(|g| {
        for id in layers.into_iter().chain([egui::LayerId::background(), egui::LayerId::debug()]) {
            if let Some(list) = g.get_mut(id) {
                *list = egui::layers::PaintList::default();
            }
        }
    });
}
