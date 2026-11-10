//! `--debug` overlay: a semi-transparent panel in the top-right corner showing the frame time,
//! approximate memory use, open drawings, the running command and where the 2D canvas's frames
//! and regenerations spend their time. Visible when
//! [`CadApp::debug_overlay`] is set (the `--debug` flag).

use crate::CadApp;

/// Show the debug overlay.
pub fn show(app: &CadApp, ctx: &egui::Context) {
    let area = egui::Area::new(egui::Id::new("cadaddy-debug-overlay"))
        .anchor(egui::Align2::RIGHT_TOP, egui::Vec2::new(-8.0, 32.0))
        .interactable(false)
        .order(egui::Order::Foreground);
    area.show(ctx, |ui| {
        egui::Frame::popup(ui.style()).fill(egui::Color32::from_black_alpha(200)).show(ui, |ui| {
            ui.set_min_width(180.0);
            let mono = egui::TextStyle::Monospace;
            ui.label(egui::RichText::new("DEBUG").color(egui::Color32::YELLOW).text_style(mono.clone()));
            ui.label(egui::RichText::new(format!("frame: {:.1} ms", app.frame_ms)).text_style(mono.clone()).color(egui::Color32::WHITE));

            if let Some(rss) = memory_shown() {
                ui.label(egui::RichText::new(format!("memory: {rss:.0} MB")).text_style(mono.clone()).color(egui::Color32::WHITE));
            }

            ui.label(egui::RichText::new(format!("drawings: {}", app.session.docs.len())).text_style(mono.clone()).color(egui::Color32::WHITE));
            let cmd = app.session.running.as_ref().map_or("-", |r| r.id.as_str());
            ui.label(egui::RichText::new(format!("command: {cmd}")).text_style(mono.clone()).color(egui::Color32::WHITE));
            if let Ok(doc) = app.session.doc() {
                // Its count kept, not counted: walking a million objects every frame was the
                // overlay's own cost (Q6482).
                let n = doc.model.len();
                ui.label(egui::RichText::new(format!("objects: {n}")).text_style(mono.clone()).color(egui::Color32::WHITE));
            }
            // Where the frame's CPU time goes, the costliest parts (`framecost`).
            for line in crate::framecost::overlay_lines(8) {
                ui.label(egui::RichText::new(line).text_style(mono.clone()).color(egui::Color32::LIGHT_GRAY));
            }
            // Where the 2D canvas's time goes (`canvas::drawstats`).
            for line in app.canvas.stats.lines(&adapter_name(app)) {
                ui.label(egui::RichText::new(line).text_style(mono.clone()).color(egui::Color32::LIGHT_GRAY));
            }
        });
    });
    ctx.request_repaint();
}

/// The GPU drawing the canvas, or that there is none.
pub fn adapter_name(app: &CadApp) -> String {
    if app.canvas.gpu.is_some() { app.gpu_caps.adapter.clone() } else { "CPU canvas".into() }
}

/// The 2D draw statistics (`ui.draw2d`, see `canvas::drawstats`).
pub fn draw2d_json(app: &CadApp) -> serde_json::Value {
    app.canvas.stats.json(&adapter_name(app), app.canvas.gpu.is_some(), crate::gpu::gpu_bytes(), process_rss_mb())
}

thread_local! {
    static MEMORY_SHOWN: std::cell::Cell<(f64, Option<f64>)> = const { std::cell::Cell::new((f64::NEG_INFINITY, None)) };
}

/// The memory the overlay shows, read at most once a second: on macOS a read starts `ps`, a
/// process per frame otherwise (Q6482).
fn memory_shown() -> Option<f64> {
    let now = crate::now_ms();
    MEMORY_SHOWN.with(|m| {
        let (at, mb) = m.get();
        if now - at < 1000.0 && now >= at {
            return mb;
        }
        let mb = process_rss_mb();
        m.set((now, mb));
        mb
    })
}

/// Approximate resident set size of this process in megabytes (safe, no FFI).
#[cfg(target_os = "macos")]
fn process_rss_mb() -> Option<f64> {
    // `ps -o rss= -p <pid>` gives RSS in kilobytes without unsafe FFI.
    let out = std::process::Command::new("ps").args(["-o", "rss=", "-p", &std::process::id().to_string()]).output().ok()?;
    let kb: f64 = String::from_utf8_lossy(&out.stdout).trim().parse().ok()?;
    Some(kb / 1024.0)
}

#[cfg(target_os = "linux")]
fn process_rss_mb() -> Option<f64> {
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    let rss_pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    let page = 4096u64;
    Some((rss_pages * page) as f64 / (1024.0 * 1024.0))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn process_rss_mb() -> Option<f64> {
    None
}
