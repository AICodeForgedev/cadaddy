//! Walk mode: the open drawing turned into 3D and walked first-person, inside CADaddy.
//!
//! The 3D is derived ([`cadcraft_walk3d`]), never modelled: walls stand up from the plan's wall
//! layers, devices hang at their mounting heights, floors stack at their elevations. The rules sit
//! beside the drawing as `<name>.walk.json`, or in a building file in the same folder that names
//! this drawing as one floor's sheet (then every floor of the building is walked). Nothing is
//! computed until Walk is opened, and all of it is dropped when Walk closes.

pub mod gpu;

use crate::CadApp;
use cadcraft_doc::{Drawing, EntityKind, Handle};
use cadcraft_walk3d::{Owner, Rules, Scene, Walker, derive_set, pick};
use egui_wgpu::wgpu;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Building files this large are not rules (hostile folder contents).
const MAX_RULES_BYTES: u64 = 4 << 20;
/// Building files looked at in one folder.
const MAX_RULES_FILES: usize = 64;

/// The drawings walked and the 3D derived from them.
pub struct Building {
    /// Index = [`Owner::src`]: 0 is the drawing Walk was opened on, then the rules' sheets.
    pub drawings: Vec<Arc<Drawing>>,
    pub rules: Rules,
    /// Where the rules came from, for the panel.
    pub rules_from: String,
    pub scene: Scene,
    /// Sheets that could not be read (their floors are empty).
    pub problems: Vec<String>,
}

impl Building {
    /// Rules for the drawing at `path`: its own `<name>.walk.json`, else a `*.walk.json` in its
    /// folder that names it as a level's sheet, else the built-in defaults. Returns the rules file.
    pub fn find_rules(path: Option<&Path>) -> Result<(Rules, Option<PathBuf>), String> {
        let Some(path) = path else { return Ok((Rules::fallback(), None)) };
        let side = Rules::sidecar_path(path);
        if let Some(s) = read_small(&side) {
            return Ok((Rules::from_json(&s).map_err(|e| format!("{}: {e}", side.display()))?, Some(side)));
        }
        let name = path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
        let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
            .map(|rd| {
                rd.filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().to_lowercase().ends_with(".walk.json")))
                    .take(MAX_RULES_FILES)
                    .collect()
            })
            .unwrap_or_default();
        files.sort();
        for f in files {
            let Some(s) = read_small(&f) else { continue };
            let Ok(rules) = Rules::from_json(&s) else { continue };
            if rules.sheets().iter().any(|sh| sheet_name(sh) == name) {
                return Ok((rules, Some(f)));
            }
        }
        Ok((Rules::fallback(), None))
    }

    /// Derive the building for `doc` (saved at `path`, if it has been). A sheet that is this
    /// drawing's own file uses the open drawing, unsaved edits included.
    pub fn load(doc: Arc<Drawing>, path: Option<&Path>) -> Result<Building, String> {
        let (rules, from) = Building::find_rules(path)?;
        Building::with_rules(doc, path, rules, from)
    }

    /// Open a drawing or a building file (`.walk.json`) from disk, without a session (the
    /// headless renderer).
    pub fn load_file(path: &Path) -> Result<Building, String> {
        if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("json")) {
            let s = read_small(path).ok_or_else(|| format!("{}: cannot read", path.display()))?;
            let rules = Rules::from_json(&s).map_err(|e| format!("{}: {e}", path.display()))?;
            return Building::with_rules(Arc::new(Drawing::default()), None, rules, Some(path.to_path_buf()));
        }
        let doc = read_drawing(path)?;
        Building::load(Arc::new(doc), Some(path))
    }

    fn with_rules(doc: Arc<Drawing>, path: Option<&Path>, rules: Rules, from: Option<PathBuf>) -> Result<Building, String> {
        let own = path.and_then(Path::file_name).map(|n| n.to_string_lossy().to_lowercase());
        let dir = from.as_deref().and_then(Path::parent).map(Path::to_path_buf).unwrap_or_default();
        let mut drawings = vec![Arc::clone(&doc)];
        let mut problems = Vec::new();
        for sheet in rules.sheets() {
            if own.as_deref() == Some(sheet_name(&sheet).as_str()) {
                drawings.push(Arc::clone(&doc));
            } else {
                match read_drawing(&dir.join(&sheet)) {
                    Ok(d) => drawings.push(Arc::new(d)),
                    Err(e) => {
                        problems.push(e);
                        drawings.push(Arc::new(Drawing::default()));
                    }
                }
            }
        }
        let scene = derive_set(&drawings.iter().map(|d| &**d).collect::<Vec<_>>(), &rules);
        let rules_from = match &from {
            Some(f) => f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            None => "built-in defaults (no .walk.json beside the drawing)".into(),
        };
        Ok(Building { drawings, rules, rules_from, scene, problems })
    }

    /// The level to start on: the first one drawn on the open drawing (opening A112 starts on
    /// Level 2), else the lowest.
    pub fn home_level(&self) -> usize {
        let open = self.drawings.first();
        (0..self.rules.levels.len())
            .find(|&i| self.drawings.get(self.rules.level_source(i)).zip(open).is_some_and(|(a, b)| Arc::ptr_eq(a, b)))
            .unwrap_or(0)
    }

    /// The sheet a source index came from, for the panel ("" for the open drawing).
    pub fn sheet_of(&self, src: u16) -> String {
        match src {
            0 => String::new(),
            n => self.rules.sheets().get(usize::from(n) - 1).cloned().unwrap_or_default(),
        }
    }
}

/// A sheet's file name, lower case (sheets may name sub-folders).
fn sheet_name(sheet: &str) -> String {
    Path::new(sheet).file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default()
}

fn read_small(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_RULES_BYTES {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

fn read_drawing(path: &Path) -> Result<Drawing, String> {
    cadcraft_engine::cmd::file::read_file(&path.to_string_lossy()).map(|f| f.into_drawing())
}

/// What the last click hit.
pub enum Picked {
    Device(usize),
    Entity(Owner),
}

/// Walk mode while it is open.
pub struct Walk {
    pub building: Building,
    pub walker: Walker,
    gpu: Option<gpu::Gpu>,
    texture: Option<egui::TextureId>,
    uploaded: bool,
    pub picked: Option<Picked>,
    /// Cut away everything above the walker's level (a dollhouse view from above).
    pub cut: bool,
    /// The open drawing's revision the scene was derived from; edits rebuild it.
    revision: u64,
    fps: f32,
}

impl Walk {
    fn new(building: Building, revision: u64) -> Walk {
        let walker = Walker::spawn(&building.scene, building.home_level());
        Walk { building, walker, gpu: None, texture: None, uploaded: false, picked: None, cut: false, revision, fps: 0.0 }
    }
}

/// Open walk mode on the active drawing.
pub fn open(app: &mut CadApp) -> Result<(), String> {
    let st = app.session.state().map_err(|e| e.to_string())?;
    let (doc, path, revision) = (Arc::clone(&st.doc), st.path.clone(), st.revision);
    let building = Building::load(doc, path.as_deref().map(Path::new))?;
    let walls = building.scene.walls.len();
    let levels = building.scene.levels.len();
    let devices = building.scene.devices.len();
    let from = building.rules_from.clone();
    let problems = building.problems.join("; ");
    app.walk = Some(Walk::new(building, revision));
    if problems.is_empty() {
        app.set_status(format!("Walk: {levels} levels, {walls} wall faces, {devices} devices (rules: {from}). Esc returns to the drawing."));
    } else {
        app.set_status(format!("Walk: some floors are empty, their sheets could not be read: {problems}"));
    }
    Ok(())
}

/// Close walk mode and free its GPU resources.
pub fn close(app: &mut CadApp) {
    if let Some(w) = app.walk.take()
        && let (Some(id), Some(rs)) = (w.texture, app.wgpu_rs.as_ref())
    {
        rs.renderer.write().free_texture(&id);
    }
}

/// Height above which nothing is drawn: off, or the top of the walker's level.
fn cut_z(scene: &Scene, w: &Walker, on: bool) -> f32 {
    match scene.levels.get(w.level) {
        Some(l) if on => (l.elevation_ft + l.height_ft - 0.05) as f32,
        _ => 1e9,
    }
}

pub fn frame_for(scene: &Scene, w: &Walker, size: (u32, u32), cut: bool) -> gpu::Frame {
    let aspect = f64::from(size.0) / f64::from(size.1.max(1));
    gpu::Frame { view_proj: w.view_proj(aspect).to_gpu(), eye: w.eye.f32(), cut_z: cut_z(scene, w, cut), fog_ft: 400.0 }
}

/// The walk view in place of the drawing canvas: the 3D on the left, its panel on the right.
pub fn show(app: &mut CadApp, ui: &mut egui::Ui) {
    refresh_if_edited(app);
    let rs = app.wgpu_rs.clone();
    let ctx = ui.ctx().clone();
    let mut close_now = false;
    let mut select: Option<Handle> = None;
    {
        let Some(walk) = app.walk.as_mut() else { return };
        let dt = ctx.input(|i| i.stable_dt);
        if dt > 0.0 {
            walk.fps = walk.fps * 0.9 + (1.0 / dt) * 0.1;
        }
        egui::Panel::right("walk_panel").default_size(270.0).show(ui, |ui| {
            close_now = side_panel(walk, ui);
        });
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
            let (esc, sel) = viewport(walk, ui, rs.as_ref());
            close_now |= esc;
            select = sel;
        });
    }
    // A wall or device of the open drawing picked in 3D is selected in 2D too.
    if let Some(h) = select
        && let Ok(st) = app.session.state_mut()
    {
        st.selection = vec![h];
    }
    if close_now {
        close(app);
    }
}

/// Rebuild the scene when the open drawing changed (an edit, an undo), keeping the walker.
fn refresh_if_edited(app: &mut CadApp) {
    let Ok(st) = app.session.state() else { return };
    let Some(walk) = app.walk.as_mut() else { return };
    if st.revision == walk.revision {
        return;
    }
    walk.revision = st.revision;
    let path = st.path.clone();
    match Building::load(Arc::clone(&st.doc), path.as_deref().map(Path::new)) {
        Ok(b) => {
            let level = walk.walker.level.min(b.scene.levels.len().saturating_sub(1));
            walk.walker.level = level;
            walk.building = b;
            walk.uploaded = false;
            walk.picked = None;
        }
        Err(e) => app.set_status(format!("Walk: {e}")),
    }
}

/// The panel beside the 3D. Returns true when the user asked to go back to the drawing.
fn side_panel(walk: &mut Walk, ui: &mut egui::Ui) -> bool {
    let mut back = false;
    ui.horizontal(|ui| {
        crate::icons::paint(ui.painter(), egui::Rect::from_min_size(ui.cursor().min, egui::vec2(22.0, 22.0)), crate::icons::Icon::Walk, false);
        ui.add_space(26.0);
        ui.heading("Walk");
    });
    if ui.button("Back to the drawing (Esc)").clicked() {
        back = true;
    }
    let b = &walk.building;
    let s = &b.scene;
    ui.small(format!("Rules: {}", b.rules_from));
    for p in &b.problems {
        ui.colored_label(egui::Color32::from_rgb(230, 160, 60), format!("Empty floor: {p}"));
    }
    ui.small(format!("{} wall faces · {} devices · {:.0} fps", s.walls.len(), s.devices.len(), walk.fps));
    ui.separator();
    ui.strong("Levels");
    let w = &mut walk.walker;
    for (i, lv) in s.levels.iter().enumerate() {
        if ui.selectable_label(w.level == i, format!("{}  ({})", lv.name, ft_in(lv.elevation_ft))).clicked() {
            w.go_to_level(s, i);
        }
    }
    ui.separator();
    if ui.checkbox(&mut w.fly, "Fly (F)").changed() && !w.fly {
        w.go_to_level(s, w.level);
    }
    ui.checkbox(&mut walk.cut, "Cut away levels above (H)");
    ui.label(format!(
        "Eye {} , {} , {} AFF",
        ft_in(w.eye.x),
        ft_in(w.eye.y),
        ft_in(w.eye.z - s.levels.get(w.level).map(|v| v.elevation_ft).unwrap_or(0.0))
    ));
    ui.separator();
    ui.strong("Picked");
    match &walk.picked {
        Some(Picked::Device(i)) => {
            if let Some(d) = s.devices.get(*i) {
                egui::Grid::new("walk_picked").num_columns(2).show(ui, |ui| {
                    ui.label("Block");
                    ui.label(&d.block);
                    ui.end_row();
                    for (k, v) in &d.attrs {
                        ui.label(k);
                        ui.label(v);
                        ui.end_row();
                    }
                    ui.label("Level");
                    ui.label(s.levels.get(d.level).map(|v| v.name.as_str()).unwrap_or("?"));
                    ui.end_row();
                    ui.label("Mount");
                    ui.label(if d.ceiling { format!("ceiling, {} AFF", ft_in(d.aff_ft)) } else { format!("wall, {} AFF", ft_in(d.aff_ft)) });
                    ui.end_row();
                    let sheet = b.sheet_of(d.src);
                    if !sheet.is_empty() {
                        ui.label("Sheet");
                        ui.label(sheet);
                        ui.end_row();
                    }
                });
            }
        }
        Some(Picked::Entity(o)) => match b.drawings.get(usize::from(o.src)).and_then(|d| d.model.get(o.handle)) {
            Some(e) => {
                let kind = match &e.kind {
                    EntityKind::LwPolyline(_) => "Polyline",
                    EntityKind::Line(_) => "Line",
                    _ => "Entity",
                };
                let sheet = b.sheet_of(o.src);
                let sheet = if sheet.is_empty() { String::new() } else { format!(" in {sheet}") };
                ui.label(format!("{kind} on {}{sheet}", e.common.layer));
            }
            None => {
                ui.label("—");
            }
        },
        None => {
            ui.small("Click a wall or device.");
        }
    }
    ui.separator();
    ui.small("Drag: look · WASD/arrows: move · Shift: run\nQ/E: down/up (fly) · PgUp/PgDn: level\nF: fly · H: cut above · click: pick · Esc: back");
    back
}

/// The 3D view: look, move, pick, draw. Returns (Esc pressed, a handle of the open drawing to
/// select).
fn viewport(walk: &mut Walk, ui: &mut egui::Ui, rs: Option<&egui_wgpu::RenderState>) -> (bool, Option<Handle>) {
    let rect = ui.available_rect_before_wrap();
    let resp = ui.allocate_rect(rect, egui::Sense::click_and_drag());
    let ctx = ui.ctx().clone();
    let Some(rs) = rs else {
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "Walk needs the GPU renderer.",
            egui::FontId::proportional(16.0),
            egui::Color32::WHITE,
        );
        return (ctx.input(|i| i.key_pressed(egui::Key::Escape)), None);
    };
    let s = &walk.building.scene;
    let w = &mut walk.walker;
    let d = resp.drag_delta();
    if resp.dragged() {
        w.look(-f64::from(d.x) * 0.005, -f64::from(d.y) * 0.005);
    }
    let mut esc = false;
    if !ctx.egui_wants_keyboard_input() {
        let (dt, keys) = ctx.input(|i| {
            let k = |a: egui::Key, b: egui::Key| f64::from(u8::from(i.key_down(a) || i.key_down(b)));
            let fwd = k(egui::Key::W, egui::Key::ArrowUp) - k(egui::Key::S, egui::Key::ArrowDown);
            let right = k(egui::Key::D, egui::Key::ArrowRight) - k(egui::Key::A, egui::Key::ArrowLeft);
            let up = k(egui::Key::E, egui::Key::E) - k(egui::Key::Q, egui::Key::Q);
            let run = if i.modifiers.shift { 3.0 } else { 1.0 };
            let toggles = (
                i.key_pressed(egui::Key::F),
                i.key_pressed(egui::Key::H),
                i.key_pressed(egui::Key::PageUp),
                i.key_pressed(egui::Key::PageDown),
                i.key_pressed(egui::Key::Escape),
            );
            (f64::from(i.stable_dt).min(0.1), (fwd, right, up, run, toggles))
        });
        let (fwd, right, up, run, (f, h, pgup, pgdn, escape)) = keys;
        let speed = if w.fly { 25.0 } else { 6.0 } * run * dt;
        if fwd != 0.0 || right != 0.0 || up != 0.0 {
            w.step(s, fwd * speed, right * speed, up * speed);
        }
        if f {
            w.fly = !w.fly;
            if !w.fly {
                w.go_to_level(s, w.level);
            }
        }
        if h {
            walk.cut = !walk.cut;
        }
        if pgup && w.level + 1 < s.levels.len() {
            w.go_to_level(s, w.level + 1);
        }
        if pgdn && w.level > 0 {
            w.go_to_level(s, w.level - 1);
        }
        esc = escape;
    }
    let aspect = f64::from(rect.width() / rect.height().max(1.0));
    let mut select = None;
    if resp.clicked()
        && let Some(p) = resp.interact_pointer_pos()
    {
        let nx = f64::from((p.x - rect.left()) / rect.width() * 2.0 - 1.0);
        let ny = f64::from(1.0 - (p.y - rect.top()) / rect.height() * 2.0);
        let dir = w.ray(nx, ny, aspect);
        walk.picked = pick(s, w.eye, dir).map(|hit| match s.devices.iter().position(|d| d.handle == hit.owner.handle && d.src == hit.owner.src) {
            Some(i) => Picked::Device(i),
            None => Picked::Entity(hit.owner),
        });
        // The open drawing is source 0, and any sheet that is the same drawing.
        if let Some(o) = match &walk.picked {
            Some(Picked::Device(i)) => s.devices.get(*i).map(|d| Owner { src: d.src, handle: d.handle }),
            Some(Picked::Entity(o)) => Some(*o),
            None => None,
        } && walk.building.drawings.get(usize::from(o.src)).zip(walk.building.drawings.first()).is_some_and(|(a, b)| Arc::ptr_eq(a, b))
        {
            select = Some(o.handle);
        }
    }
    // Draw.
    let ppp = ctx.pixels_per_point();
    let size = ((rect.width() * ppp).round().max(1.0) as u32, (rect.height() * ppp).round().max(1.0) as u32);
    let g = walk.gpu.get_or_insert_with(|| gpu::Gpu::new(&rs.device));
    if !walk.uploaded {
        g.upload(&rs.device, s);
        walk.uploaded = true;
    }
    g.render(&rs.device, &rs.queue, size, &frame_for(s, &walk.walker, size, walk.cut));
    if let Some(view) = g.view() {
        let mut r = rs.renderer.write();
        match walk.texture {
            Some(id) => r.update_egui_texture_from_wgpu_texture(&rs.device, view, wgpu::FilterMode::Linear, id),
            None => walk.texture = Some(r.register_native_texture(&rs.device, view, wgpu::FilterMode::Linear)),
        }
    }
    if let Some(id) = walk.texture {
        ui.painter().image(id, rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
    }
    ui.painter().circle_filled(rect.center(), 2.5, egui::Color32::from_rgba_unmultiplied(255, 255, 255, 200));
    if let Some(lv) = s.levels.get(walk.walker.level) {
        ui.painter().text(
            rect.left_top() + egui::vec2(10.0, 8.0),
            egui::Align2::LEFT_TOP,
            &lv.name,
            egui::FontId::proportional(18.0),
            egui::Color32::WHITE,
        );
    }
    ctx.request_repaint();
    (esc, select)
}

/// 12.5 → 12'-6"
pub fn ft_in(ft: f64) -> String {
    if !ft.is_finite() {
        return "?".into();
    }
    let sign = if ft < 0.0 { "-" } else { "" };
    let total = (ft.abs() * 12.0).round() as i64;
    format!("{sign}{}'-{}\"", total / 12, total % 12)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feet_inches() {
        assert_eq!(ft_in(19.6458), "19'-8\"");
        assert_eq!(ft_in(-1.5), "-1'-6\"");
        assert_eq!(ft_in(f64::NAN), "?");
    }

    /// A building file in the drawing's folder that lists it as a sheet is found, and the open
    /// drawing stands in for its own sheet (unsaved edits are walked).
    #[test]
    fn a_floor_sheet_finds_its_building() {
        let dir = std::env::temp_dir().join(format!("walk_building_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let rules = r#"{"levels":[{"name":"L1","elevation_ft":0,"height_ft":10,"sheet":"A111.dxf","base":[0,0]},
            {"name":"L2","elevation_ft":10,"height_ft":10,"sheet":"A112.dxf","base":[0,0]}],
            "layers":[{"layer":"A-WALL","kind":"wall"}]}"#;
        let _ = std::fs::write(dir.join("Hotel.walk.json"), rules);
        let mut d = Drawing::default();
        let mut e = cadcraft_doc::Entity::new(
            Handle(5),
            EntityKind::Line(cadcraft_doc::Line {
                a: cadcraft_geom::Vec3 { x: 0.0, y: 0.0, z: 0.0 },
                b: cadcraft_geom::Vec3 { x: 120.0, y: 0.0, z: 0.0 },
            }),
        );
        e.common.layer = "A-WALL".into();
        d.model.push(e);
        let doc = Arc::new(d);
        let b = Building::load(Arc::clone(&doc), Some(&dir.join("a112.DXF"))).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(b.rules_from, "Hotel.walk.json");
        assert_eq!(b.scene.walls.iter().map(|w| w.level).collect::<Vec<_>>(), [1]);
        assert!(b.drawings.get(2).is_some_and(|s| Arc::ptr_eq(s, &doc)), "the open drawing is its sheet");
        assert_eq!(b.home_level(), 1, "walking A112 starts on its floor");
        // A111 isn't there: its floor is empty and reported, the rest still walks.
        assert!(b.problems.len() == 1 && b.problems.iter().all(|p| p.contains("A111")), "{:?}", b.problems);
        // A drawing the building doesn't list gets the defaults.
        let other = Building::load(Arc::clone(&doc), Some(&dir.join("E-4.dxf"))).unwrap_or_else(|e| panic!("{e}"));
        assert!(other.rules_from.starts_with("built-in"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
