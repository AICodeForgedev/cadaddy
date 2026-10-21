//! `--debug` overlay: a semi-transparent panel in the top-right corner showing the frame time,
//! approximate memory use, open drawings and the running command. Visible when
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

            if let Some(rss) = process_rss_mb() {
                ui.label(egui::RichText::new(format!("memory: {rss:.0} MB")).text_style(mono.clone()).color(egui::Color32::WHITE));
            }

            ui.label(egui::RichText::new(format!("drawings: {}", app.session.docs.len())).text_style(mono.clone()).color(egui::Color32::WHITE));
            let cmd = app.session.running.as_ref().map_or("-", |r| r.id.as_str());
            ui.label(egui::RichText::new(format!("command: {cmd}")).text_style(mono.clone()).color(egui::Color32::WHITE));
            if let Ok(doc) = app.session.doc() {
                let n = doc.model.iter().count();
                ui.label(egui::RichText::new(format!("objects: {n}")).text_style(mono).color(egui::Color32::WHITE));
            }
        });
    });
    ctx.request_repaint();
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
