//! The Render window: the last RENDER's image, how long it took, and Save. It opens by itself
//! when a render finishes (from the menu, the command line or a script).

use egui::TextureHandle;

use crate::CadApp;

#[derive(Clone)]
struct Shown {
    serial: u64,
    tex: TextureHandle,
    info: String,
}

pub fn show(app: &mut CadApp, ctx: &egui::Context) {
    let id = egui::Id::new("cc_render_window");
    let open_id = egui::Id::new("cc_render_window_open");
    let Some(r) = app.session.last_render.clone() else { return };
    let shown = ctx.data(|d| d.get_temp::<Shown>(id));
    let shown = match shown {
        Some(s) if s.serial == r.serial => s,
        _ => {
            let size = [r.image.width as usize, r.image.height as usize];
            let tex = ctx.load_texture("render-window", egui::ColorImage::from_rgba_unmultiplied(size, &r.image.rgba), egui::TextureOptions::LINEAR);
            let info = format!(
                "{} × {} · {} {} · {:.2} s ({:.1} ms {})",
                r.image.width,
                r.image.height,
                r.samples,
                crate::i18n::t("samples"),
                r.seconds,
                r.seconds_per_sample * 1000.0,
                crate::i18n::t("a sample")
            );
            let s = Shown { serial: r.serial, tex, info };
            ctx.data_mut(|d| {
                d.insert_temp(id, s.clone());
                d.insert_temp(open_id, true);
            });
            s
        }
    };
    let mut open = ctx.data(|d| d.get_temp::<bool>(open_id)).unwrap_or(false);
    if !open {
        return;
    }
    let mut save = false;
    egui::Window::new(crate::tl!("Render")).id(id).default_pos(crate::palette_pos(app)).open(&mut open).resizable(true).default_width(820.0).show(
        ctx,
        |ui| {
            ui.label(&shown.info);
            let size = shown.tex.size_vec2();
            let avail = ui.available_size().max(egui::vec2(200.0, 150.0));
            let k = (avail.x / size.x).min((avail.y - 40.0).max(100.0) / size.y).min(1.0);
            ui.image((shown.tex.id(), size * k));
            ui.horizontal(|ui| {
                save = ui.button(crate::tl!("Save...")).clicked();
            });
        },
    );
    if save
        && let Some(pick) = &app.services.pick_save_image
        && let Some(path) = pick("render.png")
        && let Err(e) = app.run("saveimg", serde_json::json!({ "path": path }))
    {
        app.session.echo(e);
    }
    ctx.data_mut(|d| d.insert_temp(open_id, open));
}
