//! CADaddy Walk: open a drawing, derive its 3D model from the plans (levels + layer rules in
//! `<drawing>.walk.json`), and walk it.
//!
//! `cadaddy-walk [FILE] [--rules R.json] [--level N]` opens a window.
//! `cadaddy-walk FILE --shot OUT.png [--cam x,y,z,yaw°,pitch°] [--size WxH] [--cut]` renders one
//! frame without a window.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

mod gpu;

use cadcraft_doc::{Drawing, EntityKind, Handle};
use cadcraft_walk3d::{Rules, Scene, V3, Walker, derive, pick};
use egui_wgpu::wgpu;
use std::path::{Path, PathBuf};

struct Loaded {
    path: PathBuf,
    drawing: Drawing,
    rules: Rules,
    /// Where the rules came from (a sidecar file, or the built-in fallback).
    rules_from: String,
    scene: Scene,
}

fn load(path: &Path, rules_path: Option<&Path>) -> Result<Loaded, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let drawing = cadcraft_io::read(&bytes, &name).map_err(|e| format!("{name}: {e}"))?;
    let side = rules_path.map(Path::to_path_buf).unwrap_or_else(|| Rules::sidecar_path(path));
    let (rules, rules_from) = match std::fs::read_to_string(&side) {
        Ok(s) => (Rules::from_json(&s)?, side.display().to_string()),
        Err(_) if rules_path.is_none() => (Rules::fallback(), "built-in defaults (no .walk.json beside the drawing)".into()),
        Err(e) => return Err(format!("{}: {e}", side.display())),
    };
    let scene = derive(&drawing, &rules);
    Ok(Loaded { path: path.to_path_buf(), drawing, rules, rules_from, scene })
}

struct Args {
    file: Option<PathBuf>,
    rules: Option<PathBuf>,
    shot: Option<PathBuf>,
    cam: Option<[f64; 5]>,
    size: (u32, u32),
    level: usize,
    cut: bool,
}

fn args() -> Result<Args, String> {
    let mut a = Args { file: None, rules: None, shot: None, cam: None, size: (1600, 900), level: 0, cut: false };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut val = || it.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--rules" => a.rules = Some(val()?.into()),
            "--shot" => a.shot = Some(val()?.into()),
            "--level" => a.level = val()?.parse().map_err(|_| "--level N")?,
            "--cut" => a.cut = true,
            "--size" => {
                let v = val()?;
                let (w, h) = v.split_once('x').ok_or("--size WxH")?;
                a.size = (w.parse().map_err(|_| "--size WxH")?, h.parse().map_err(|_| "--size WxH")?);
            }
            "--cam" => {
                let v: Vec<f64> = val()?.split(',').filter_map(|s| s.trim().parse().ok()).collect();
                let [x, y, z, yaw, pitch] = v.as_slice() else { return Err("--cam x,y,z,yaw,pitch (feet, degrees)".into()) };
                a.cam = Some([*x, *y, *z, *yaw, *pitch]);
            }
            _ if !arg.starts_with("--") => a.file = Some(arg.into()),
            _ => return Err(format!("unknown option {arg}")),
        }
    }
    Ok(a)
}

fn main() {
    let result = args().and_then(|a| if a.shot.is_some() { shot(&a) } else { window(a) });
    if let Err(e) = result {
        eprintln!("cadaddy-walk: {e}");
        std::process::exit(1);
    }
}

/// Height above which nothing is drawn: off, or the top of the walker's level.
fn cut_z(scene: &Scene, w: &Walker, on: bool) -> f32 {
    match scene.levels.get(w.level) {
        Some(l) if on => (l.elevation_ft + l.height_ft - 0.05) as f32,
        _ => 1e9,
    }
}

fn frame_for(scene: &Scene, w: &Walker, size: (u32, u32), cut: bool) -> gpu::Frame {
    let aspect = f64::from(size.0) / f64::from(size.1.max(1));
    gpu::Frame { view_proj: w.view_proj(aspect).to_gpu(), eye: w.eye.f32(), cut_z: cut_z(scene, w, cut), fog_ft: 400.0 }
}

fn shot(a: &Args) -> Result<(), String> {
    let file = a.file.as_deref().ok_or("--shot needs a drawing")?;
    let out = a.shot.as_deref().ok_or("--shot OUT.png")?;
    let l = load(file, a.rules.as_deref())?;
    let mut w = Walker::spawn(&l.scene, a.level);
    if let Some([x, y, z, yaw, pitch]) = a.cam {
        w.eye = V3 { x, y, z };
        w.yaw = yaw.to_radians();
        w.pitch = pitch.to_radians();
    }
    let instance = wgpu::Instance::default();
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).map_err(|e| format!("no GPU adapter: {e}"))?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).map_err(|e| format!("GPU device: {e}"))?;
    let mut g = gpu::Gpu::new(&device);
    g.upload(&device, &l.scene);
    g.render(&device, &queue, a.size, &frame_for(&l.scene, &w, a.size, a.cut));
    let (width, height, rgba) = g.read_rgba(&device, &queue)?;
    let img = image::RgbaImage::from_raw(width, height, rgba).ok_or("bad image size")?;
    img.save(out).map_err(|e| format!("{}: {e}", out.display()))?;
    println!(
        "{}: {} levels, {} wall faces, {} devices, {} unplaced; eye {:.1},{:.1},{:.1} -> {}",
        l.path.display(),
        l.scene.levels.len(),
        l.scene.walls.len(),
        l.scene.devices.len(),
        l.scene.unplaced,
        w.eye.x,
        w.eye.y,
        w.eye.z,
        out.display()
    );
    Ok(())
}

fn window(a: Args) -> Result<(), String> {
    let loaded = match &a.file {
        Some(f) => Some(load(f, a.rules.as_deref())?),
        None => None,
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title("CADaddy Walk").with_inner_size([1500.0, 920.0]),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    let level = a.level;
    eframe::run_native(
        "CADaddy Walk",
        options,
        Box::new(move |cc| {
            let mut app = App { loaded: None, walker: None, gpu: None, texture: None, picked: None, cut: false, error: None, fps: 0.0 };
            if let Some(rs) = cc.wgpu_render_state.as_ref() {
                app.gpu = Some(gpu::Gpu::new(&rs.device));
            }
            if let Some(l) = loaded {
                app.set(l, level, cc.wgpu_render_state.as_ref());
            }
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| e.to_string())
}

/// What the last click hit.
enum Picked {
    Device(usize),
    Entity(Handle),
}

struct App {
    loaded: Option<Loaded>,
    walker: Option<Walker>,
    gpu: Option<gpu::Gpu>,
    texture: Option<egui::TextureId>,
    picked: Option<Picked>,
    cut: bool,
    error: Option<String>,
    fps: f32,
}

impl App {
    fn set(&mut self, l: Loaded, level: usize, rs: Option<&egui_wgpu::RenderState>) {
        if let (Some(g), Some(rs)) = (&mut self.gpu, rs) {
            g.upload(&rs.device, &l.scene);
        }
        self.walker = Some(Walker::spawn(&l.scene, level));
        self.picked = None;
        self.loaded = Some(l);
    }

    fn open(&mut self, path: &Path, rs: Option<&egui_wgpu::RenderState>) {
        match load(path, None) {
            Ok(l) => {
                self.error = None;
                self.set(l, 0, rs);
            }
            Err(e) => self.error = Some(e),
        }
    }

    fn side_panel(&mut self, ui: &mut egui::Ui, rs: Option<&egui_wgpu::RenderState>) {
        ui.heading("CADaddy Walk");
        if ui.button("Open drawing…").clicked()
            && let Some(p) = rfd::FileDialog::new().add_filter("Drawings", &["dxf", "dwg"]).pick_file()
        {
            self.open(&p, rs);
        }
        if let Some(e) = &self.error {
            ui.colored_label(egui::Color32::from_rgb(200, 60, 50), e);
        }
        let (Some(l), Some(w)) = (&self.loaded, &mut self.walker) else {
            ui.label("Open a DXF or DWG to walk it.");
            return;
        };
        ui.label(l.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
        ui.small(format!("Rules: {}", l.rules_from));
        ui.small(format!(
            "{} wall faces · {} devices · {} unplaced · {:.0} fps",
            l.scene.walls.len(),
            l.scene.devices.len(),
            l.scene.unplaced,
            self.fps
        ));
        ui.separator();
        ui.strong("Levels");
        for (i, lv) in l.scene.levels.iter().enumerate() {
            let label = format!("{}  ({})", lv.name, ft_in(lv.elevation_ft));
            if ui.selectable_label(w.level == i, label).clicked() {
                w.go_to_level(&l.scene, i);
            }
        }
        ui.separator();
        ui.checkbox(&mut w.fly, "Fly (F)");
        ui.checkbox(&mut self.cut, "Cut away levels above (H)");
        ui.label(format!(
            "Eye {} , {} , {} AFF",
            ft_in(w.eye.x),
            ft_in(w.eye.y),
            ft_in(w.eye.z - l.scene.levels.get(w.level).map(|v| v.elevation_ft).unwrap_or(0.0))
        ));
        ui.separator();
        ui.strong("Picked");
        match &self.picked {
            Some(Picked::Device(i)) => {
                if let Some(d) = l.scene.devices.get(*i) {
                    egui::Grid::new("picked").num_columns(2).show(ui, |ui| {
                        ui.label("Block");
                        ui.label(&d.block);
                        ui.end_row();
                        for (k, v) in &d.attrs {
                            ui.label(k);
                            ui.label(v);
                            ui.end_row();
                        }
                        ui.label("Level");
                        ui.label(l.scene.levels.get(d.level).map(|v| v.name.as_str()).unwrap_or("?"));
                        ui.end_row();
                        ui.label("Mount");
                        ui.label(if d.ceiling { format!("ceiling, {} AFF", ft_in(d.aff_ft)) } else { format!("wall, {} AFF", ft_in(d.aff_ft)) });
                        ui.end_row();
                        ui.label("Handle");
                        ui.label(format!("{:X}", d.handle.0));
                        ui.end_row();
                    });
                }
            }
            Some(Picked::Entity(h)) => match l.drawing.model.get(*h) {
                Some(e) => {
                    let kind = match &e.kind {
                        EntityKind::LwPolyline(_) => "Polyline",
                        EntityKind::Line(_) => "Line",
                        _ => "Entity",
                    };
                    ui.label(format!("{kind} on {} (handle {:X})", e.common.layer, h.0));
                    if let Some(r) = l.rules.layer(&e.common.layer) {
                        ui.small(format!("Rule: {r:?}"));
                    }
                }
                None => {
                    ui.label("—");
                }
            },
            None => {
                ui.small("Click a device or wall.");
            }
        }
        ui.separator();
        ui.small("Drag: look · WASD/arrows: move · Shift: run\nQ/E: down/up (fly) · PgUp/PgDn: level\nF: fly · H: cut above · click: pick");
    }

    fn viewport(&mut self, ui: &mut egui::Ui, rs: Option<&egui_wgpu::RenderState>) {
        let rect = ui.available_rect_before_wrap();
        let resp = ui.allocate_rect(rect, egui::Sense::click_and_drag());
        let (Some(l), Some(w)) = (&self.loaded, &mut self.walker) else { return };
        let ctx = ui.ctx().clone();
        // Look.
        let d = resp.drag_delta();
        if resp.dragged() {
            w.look(-f64::from(d.x) * 0.005, -f64::from(d.y) * 0.005);
        }
        // Move.
        if !ctx.egui_wants_keyboard_input() {
            let (dt, keys) = ctx.input(|i| {
                let k = |a: egui::Key, b: egui::Key| f64::from(u8::from(i.key_down(a) || i.key_down(b)));
                let fwd = k(egui::Key::W, egui::Key::ArrowUp) - k(egui::Key::S, egui::Key::ArrowDown);
                let right = k(egui::Key::D, egui::Key::ArrowRight) - k(egui::Key::A, egui::Key::ArrowLeft);
                let up = k(egui::Key::E, egui::Key::E) - k(egui::Key::Q, egui::Key::Q);
                let run = if i.modifiers.shift { 3.0 } else { 1.0 };
                let toggles =
                    (i.key_pressed(egui::Key::F), i.key_pressed(egui::Key::H), i.key_pressed(egui::Key::PageUp), i.key_pressed(egui::Key::PageDown));
                (f64::from(i.stable_dt).min(0.1), (fwd, right, up, run, toggles))
            });
            let (fwd, right, up, run, (f, h, pgup, pgdn)) = keys;
            let speed = if w.fly { 25.0 } else { 6.0 } * run * dt;
            if fwd != 0.0 || right != 0.0 || up != 0.0 {
                w.step(&l.scene, fwd * speed, right * speed, up * speed);
            }
            if f {
                w.fly = !w.fly;
                if !w.fly {
                    w.go_to_level(&l.scene, w.level);
                }
            }
            if h {
                self.cut = !self.cut;
            }
            if pgup && w.level + 1 < l.scene.levels.len() {
                w.go_to_level(&l.scene, w.level + 1);
            }
            if pgdn && w.level > 0 {
                w.go_to_level(&l.scene, w.level - 1);
            }
        }
        // Pick.
        let aspect = f64::from(rect.width() / rect.height().max(1.0));
        if resp.clicked()
            && let Some(p) = resp.interact_pointer_pos()
        {
            let nx = f64::from((p.x - rect.left()) / rect.width() * 2.0 - 1.0);
            let ny = f64::from(1.0 - (p.y - rect.top()) / rect.height() * 2.0);
            let dir = w.ray(nx, ny, aspect);
            self.picked = pick(&l.scene, w.eye, dir).map(|hit| match l.scene.devices.iter().position(|d| d.handle == hit.handle) {
                Some(i) => Picked::Device(i),
                None => Picked::Entity(hit.handle),
            });
        }
        // Draw.
        let ppp = ctx.pixels_per_point();
        let size = ((rect.width() * ppp).round().max(1.0) as u32, (rect.height() * ppp).round().max(1.0) as u32);
        if let (Some(g), Some(rs)) = (&mut self.gpu, rs) {
            g.render(&rs.device, &rs.queue, size, &frame_for(&l.scene, w, size, self.cut));
            if let Some(view) = g.view() {
                let mut r = rs.renderer.write();
                match self.texture {
                    Some(id) => r.update_egui_texture_from_wgpu_texture(&rs.device, view, wgpu::FilterMode::Linear, id),
                    None => self.texture = Some(r.register_native_texture(&rs.device, view, wgpu::FilterMode::Linear)),
                }
            }
        }
        if let Some(id) = self.texture {
            ui.painter().image(id, rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
        }
        let c = rect.center();
        ui.painter().circle_filled(c, 2.5, egui::Color32::from_rgba_unmultiplied(255, 255, 255, 200));
        if let Some(lv) = l.scene.levels.get(w.level) {
            ui.painter().text(
                rect.left_top() + egui::vec2(10.0, 8.0),
                egui::Align2::LEFT_TOP,
                &lv.name,
                egui::FontId::proportional(18.0),
                egui::Color32::WHITE,
            );
        }
        ctx.request_repaint();
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let rs = frame.wgpu_render_state().cloned();
        let dt = ui.ctx().input(|i| i.stable_dt);
        if dt > 0.0 {
            self.fps = self.fps * 0.9 + (1.0 / dt) * 0.1;
        }
        let dropped: Option<PathBuf> =
            ui.ctx().input(|i| i.raw.dropped_files.first().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()));
        if let Some(p) = dropped {
            self.open(&p, rs.as_ref());
        }
        egui::Panel::left("walk_side").default_size(280.0).show(ui, |ui| self.side_panel(ui, rs.as_ref()));
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| self.viewport(ui, rs.as_ref()));
    }
}

/// 12.5 → 12'-6"
fn ft_in(ft: f64) -> String {
    if !ft.is_finite() {
        return "?".into();
    }
    let sign = if ft < 0.0 { "-" } else { "" };
    let total = (ft.abs() * 12.0).round() as i64;
    format!("{sign}{}'-{}\"", total / 12, total % 12)
}

#[cfg(test)]
mod tests {
    #[test]
    fn feet_inches() {
        assert_eq!(super::ft_in(19.6458), "19'-8\"");
        assert_eq!(super::ft_in(-1.5), "-1'-6\"");
        assert_eq!(super::ft_in(f64::NAN), "?");
    }
}
