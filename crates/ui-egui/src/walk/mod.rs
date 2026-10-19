//! Walk mode (View ▸ Walk, 3DWALK and 3DFLY): the open drawing turned into 3D and walked
//! first-person in the drawing's own tab, the palettes beside it. Walking stays on what is
//! underfoot (stairs and ramps, gravity off edges), walls and shut doors stop it; a click on a door
//! swings it; a minimap shows where you are, and a room name takes you there. The speed is
//! STEPSIZE × STEPSPERSEC drawing units a second when the drawing sets them.
//!
//! The 3D is derived ([`cadaddy_walk3d`]), never modelled: walls stand up from the plan's wall
//! layers, devices hang at their mounting heights, floors stack at their elevations. The rules sit
//! beside the drawing as `<name>.walk.json`, or in a building file in the same folder that names
//! this drawing as one floor's sheet (then every floor of the building is walked). Nothing is
//! computed until Walk is opened, and all of it is dropped when Walk closes.

pub mod gpu;

use crate::CadApp;
use cadaddy_doc::{Drawing, EntityKind, Handle};
use cadaddy_walk3d::{DeviceInfo, DimKind, Ground, Owner, Room, Rules, Scene, Walker, derive_set, measure, pick, rooms};
use egui_wgpu::wgpu;
use std::collections::BTreeSet;
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

    /// Derive the building for `doc` (saved at `path`, if it has been): from the drawing's
    /// building spec when it has one (its stairs and ramps included), else from the rules file.
    /// A sheet that is this drawing's own file uses the open drawing, unsaved edits included.
    pub fn load(doc: Arc<Drawing>, path: Option<&Path>) -> Result<Building, String> {
        if let Some(spec) = doc.building_spec.as_deref().and_then(|t| cadaddy_walk3d::BuildingSpec::from_json(t).ok()) {
            let mut b = Building::with_rules(doc, path, spec.to_rules(), path.map(Path::to_path_buf))?;
            b.rules_from = "the drawing's building spec".into();
            return Ok(b);
        }
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
    cadaddy_engine::cmd::file::read_file(&path.to_string_lossy()).map(|f| f.into_drawing())
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
    /// Device types hidden in the Layers list, and whether walls show.
    pub hidden_types: BTreeSet<String>,
    pub walls_on: bool,
    /// The scene as drawn and picked: the building's, less hidden device types (and walls).
    view: Scene,
    /// Device types with their counts and colours, most common first.
    types: Vec<(String, usize, [u8; 3])>,
    /// What the walker stands on.
    ground: Ground,
    /// Room names on the plans, and the filter typed to find one.
    pub rooms: Vec<Room>,
    room_query: String,
    /// The plan of the walker's level with its position, in a corner of the view.
    pub minimap: bool,
    /// Walking speed, feet per second (flying is four times faster; Shift runs).
    pub speed_ft: f64,
}

/// Walking pace when the drawing doesn't set STEPSIZE and STEPSPERSEC, feet per second.
const WALK_PACE_FT: f64 = 4.5;

/// The walking speed for `b`: STEPSIZE × STEPSPERSEC drawing units a second when the drawing sets
/// STEPSIZE, else a walking pace.
fn speed_of(b: &Building) -> f64 {
    let Some(d) = b.drawings.first() else { return WALK_PACE_FT };
    if d.header.get("STEPSIZE").is_none() && d.header.get("STEPSPERSEC").is_none() {
        return WALK_PACE_FT;
    }
    let upf = if b.rules.units_per_foot.is_finite() && b.rules.units_per_foot > 0.0 { b.rules.units_per_foot } else { 12.0 };
    let v = d.header.f64("STEPSIZE", 6.0).clamp(1e-6, 1e6) * d.header.f64("STEPSPERSEC", 2.0).clamp(1.0, 30.0) / upf;
    if v.is_finite() { v.clamp(0.05, 500.0) } else { WALK_PACE_FT }
}

impl Walk {
    fn new(building: Building, revision: u64) -> Walk {
        let walker = Walker::spawn(&building.scene, building.home_level());
        let mut w = Walk {
            building,
            walker,
            gpu: None,
            texture: None,
            uploaded: false,
            picked: None,
            cut: false,
            revision,
            fps: 0.0,
            hidden_types: BTreeSet::new(),
            walls_on: true,
            view: Scene::default(),
            types: Vec::new(),
            ground: Ground::default(),
            rooms: Vec::new(),
            room_query: String::new(),
            minimap: true,
            speed_ft: WALK_PACE_FT,
        };
        w.rebuild_view();
        w.rebuild_ground();
        w
    }

    /// The ground, the room names and the speed, after the building is derived again.
    fn rebuild_ground(&mut self) {
        let b = &self.building;
        self.ground = Ground::new(&b.scene);
        let srcs: Vec<&Drawing> = b.drawings.iter().map(|d| d.as_ref()).collect();
        self.rooms = rooms(&srcs, &b.rules);
        self.rooms.sort_by(|a, b| a.level.cmp(&b.level).then_with(|| a.name.cmp(&b.name)));
        self.speed_ft = speed_of(b);
    }

    /// Stand at room `i`: its level, its place, on the floor there.
    pub fn go_to_room(&mut self, i: usize) {
        let Some(r) = self.rooms.get(i).cloned() else { return };
        let s = &self.building.scene;
        self.walker.go_to_level(s, r.level);
        let floor = s.levels.get(r.level).map_or(0.0, |l| l.elevation_ft);
        let z = self.ground.below(r.at, floor + self.walker.max_step).unwrap_or(floor);
        self.walker.eye = cadaddy_walk3d::V3 { x: r.at.x, y: r.at.y, z: z + self.walker.eye_height };
        self.walker.vz = 0.0;
    }

    /// Swing the door the pick `hit` landed on (its leaf nearest the point). True when one swung.
    fn toggle_door_at(&mut self, owner: Owner, point: cadaddy_walk3d::V3) -> bool {
        let s = &self.building.scene;
        let p = cadaddy_doc::geom::Vec2 { x: point.x, y: point.y };
        let best = s
            .door_leaves
            .iter()
            .enumerate()
            .filter(|(_, d)| d.owner == owner)
            .map(|(i, d)| {
                let (a, b) = d.line();
                let ab = b - a;
                let l2 = ab.x * ab.x + ab.y * ab.y;
                let t = if l2 > 1e-12 { (((p - a).x * ab.x + (p - a).y * ab.y) / l2).clamp(0.0, 1.0) } else { 0.0 };
                (i, (a + ab * t - p).len())
            })
            .min_by(|x, y| x.1.total_cmp(&y.1));
        match best {
            Some((i, dist)) if dist < 1.0 => {
                self.building.scene.toggle_door(i);
                self.rebuild_view();
                true
            }
            _ => false,
        }
    }

    /// Recount the device types and redo the drawn scene after a layer toggle or a rebuild.
    fn rebuild_view(&mut self) {
        let b = &self.building;
        let mut colour: std::collections::HashMap<(u16, u64), [u8; 3]> = std::collections::HashMap::new();
        for (i, tri) in b.scene.tris.as_chunks::<3>().0.iter().enumerate() {
            if let (Some(o), [v, ..]) = (b.scene.owners.get(i), tri) {
                colour.entry((o.src, o.handle.0)).or_insert([v.c[0], v.c[1], v.c[2]]);
            }
        }
        let mut types: std::collections::BTreeMap<String, (usize, [u8; 3])> = std::collections::BTreeMap::new();
        for d in &b.scene.devices {
            let e = types.entry(device_type(b, d)).or_insert((0, colour.get(&(d.src, d.handle.0)).copied().unwrap_or([230, 120, 30])));
            e.0 += 1;
        }
        let mut types: Vec<(String, usize, [u8; 3])> = types.into_iter().map(|(t, (n, c))| (t, n, c)).collect();
        types.sort_by(|x, y| y.1.cmp(&x.1).then_with(|| x.0.cmp(&y.0)));
        self.types = types;
        self.view = visible(b, &self.hidden_types, self.walls_on);
        self.uploaded = false;
    }
}

/// A device's type: its type attribute (as the rules name it), else its block name.
fn device_type(b: &Building, d: &DeviceInfo) -> String {
    d.attrs.iter().find(|(t, _)| t.eq_ignore_ascii_case(&b.rules.devices.type_attr)).map(|(_, v)| v.clone()).unwrap_or_else(|| d.block.clone())
}

/// The building's scene without the hidden device types, and without walls (faces, caps and
/// outlines) when they are off. Floors, ceilings and shown devices stay.
fn visible(b: &Building, hidden: &BTreeSet<String>, walls_on: bool) -> Scene {
    let s = &b.scene;
    let device_type_of: std::collections::HashMap<(u16, u64), String> = s.devices.iter().map(|d| ((d.src, d.handle.0), device_type(b, d))).collect();
    let mut v = Scene::default();
    for (i, tri) in s.tris.as_chunks::<3>().0.iter().enumerate() {
        let Some(o) = s.owners.get(i) else { break };
        let keep = match device_type_of.get(&(o.src, o.handle.0)) {
            Some(t) => !hidden.contains(t),
            None => walls_on || o.handle == Handle(0),
        };
        if keep {
            v.tris.extend_from_slice(tri);
            v.owners.push(*o);
        }
    }
    if walls_on {
        v.edges = s.edges.clone();
    }
    v
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
            close_now = egui::ScrollArea::vertical().id_salt("walk_side").show(ui, |ui| side_panel(walk, ui)).inner;
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
            walk.picked = None;
            walk.rebuild_view();
            walk.rebuild_ground();
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
    ui.checkbox(&mut walk.minimap, "Plan in the corner (M)");
    ui.small(format!("Speed {}/s{}", ft_in(walk.speed_ft), if w.fly { " (×4 flying)" } else { "" }));
    ui.label(format!(
        "Eye {} , {} , {} AFF",
        ft_in(w.eye.x),
        ft_in(w.eye.y),
        ft_in(w.eye.z - s.levels.get(w.level).map(|v| v.elevation_ft).unwrap_or(0.0))
    ));
    ui.separator();
    ui.strong("Go to room");
    ui.add(egui::TextEdit::singleline(&mut walk.room_query).hint_text("Room name").desired_width(f32::INFINITY));
    let q = walk.room_query.to_lowercase();
    let mut go: Option<usize> = None;
    egui::ScrollArea::vertical().id_salt("walk_rooms").max_height(120.0).show(ui, |ui| {
        for (i, r) in walk.rooms.iter().enumerate().filter(|(_, r)| q.is_empty() || r.name.to_lowercase().contains(&q)).take(60) {
            let lv = s.levels.get(r.level).map(|l| l.name.as_str()).unwrap_or("?");
            if ui.selectable_label(false, format!("{}  ·  {lv}", r.name)).clicked() {
                go = Some(i);
            }
        }
        if walk.rooms.is_empty() {
            ui.small("No room names on the plans.");
        }
    });
    if let Some(i) = go {
        walk.go_to_room(i);
    }
    let b = &walk.building;
    let s = &b.scene;
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
                    let to: Vec<String> = measure(s, *i).iter().filter(|m| m.kind == DimKind::ToWall).map(|m| ft_in(m.feet)).collect();
                    if !to.is_empty() {
                        ui.label(if d.ceiling { "To walls" } else { "Along wall" });
                        ui.label(to.join("  ·  "));
                        ui.end_row();
                    }
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
    if layers_panel(walk, ui) {
        walk.rebuild_view();
    }
    ui.separator();
    ui.small("Drag: look · WASD/arrows: move · Shift: run\nQ/E: down/up (fly) · PgUp/PgDn: level\nF: fly · H: cut above · M: plan\nclick: pick, or swing a door · Esc: back");
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
    let ground = &walk.ground;
    let w = &mut walk.walker;
    let d = resp.drag_delta();
    if resp.dragged() {
        w.look(-f64::from(d.x) * 0.005, -f64::from(d.y) * 0.005);
    }
    // Gravity: off an edge, the walker comes down.
    let frame_dt = ctx.input(|i| f64::from(i.stable_dt).min(0.1));
    w.fall(s, Some(ground), frame_dt);
    let mut esc = false;
    let mut minimap_key = false;
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
                i.key_pressed(egui::Key::M),
            );
            (f64::from(i.stable_dt).min(0.1), (fwd, right, up, run, toggles))
        });
        let (fwd, right, up, run, (f, h, pgup, pgdn, escape, m)) = keys;
        minimap_key = m;
        let speed = walk.speed_ft * if w.fly { 4.0 } else { 1.0 } * run * dt;
        if fwd != 0.0 || right != 0.0 || up != 0.0 {
            w.step_on(s, Some(ground), fwd * speed, right * speed, up * speed);
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
    if minimap_key {
        walk.minimap = !walk.minimap;
    }
    let map_rect = walk.minimap.then(|| minimap_rect(rect));
    let aspect = f64::from(rect.width() / rect.height().max(1.0));
    let mut select = None;
    let clicked_at = resp.clicked().then(|| resp.interact_pointer_pos()).flatten();
    // A click on the plan in the corner takes the walker there.
    if let (Some(p), Some(mr)) = (clicked_at, map_rect)
        && mr.contains(p)
    {
        if let Some(to) = minimap_to_plan(&walk.building.scene, walk.walker.level, mr, p) {
            let w = &mut walk.walker;
            w.eye.x = to.x;
            w.eye.y = to.y;
            let floor = walk.building.scene.levels.get(w.level).map_or(0.0, |l| l.elevation_ft);
            let z = walk.ground.below(to, floor + w.max_step).unwrap_or(floor);
            w.eye.z = z + w.eye_height;
            w.vz = 0.0;
        }
    } else if let Some(p) = clicked_at {
        let w = &walk.walker;
        let s = &walk.building.scene;
        let nx = f64::from((p.x - rect.left()) / rect.width() * 2.0 - 1.0);
        let ny = f64::from(1.0 - (p.y - rect.top()) / rect.height() * 2.0);
        let dir = w.ray(nx, ny, aspect);
        let hit = pick(&walk.view, w.eye, dir);
        let door = hit.as_ref().is_some_and(|h| s.door_leaves.iter().any(|d| d.owner == h.owner));
        if let Some(h) = hit.as_ref().filter(|_| door)
            && walk.toggle_door_at(h.owner, h.point)
        {
            walk.picked = None;
        } else {
            let s = &walk.building.scene;
            walk.picked = hit.map(|hit| match s.devices.iter().position(|d| d.handle == hit.owner.handle && d.src == hit.owner.src) {
                Some(i) => Picked::Device(i),
                None => Picked::Entity(hit.owner),
            });
        }
        let s = &walk.building.scene;
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
    // Draw (a door may have swung: the scene is looked at again).
    let s = &walk.building.scene;
    let ppp = ctx.pixels_per_point();
    let size = ((rect.width() * ppp).round().max(1.0) as u32, (rect.height() * ppp).round().max(1.0) as u32);
    let g = walk.gpu.get_or_insert_with(|| gpu::Gpu::new(&rs.device));
    if !walk.uploaded {
        g.upload(&rs.device, &walk.view);
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
    if let Some(Picked::Device(i)) = &walk.picked {
        draw_dims(ui.painter(), rect, &walk.walker.view_proj(aspect), &measure(&walk.building.scene, *i));
    }
    ui.painter().circle_filled(rect.center(), 2.5, egui::Color32::from_rgba_unmultiplied(255, 255, 255, 200));
    if let Some(mr) = map_rect {
        draw_minimap(ui.painter(), mr, &walk.building.scene, &walk.walker);
    }
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

/// Where the plan sits in the view: the lower-left corner.
fn minimap_rect(view: egui::Rect) -> egui::Rect {
    let side = (view.width().min(view.height()) * 0.3).clamp(120.0, 240.0);
    egui::Rect::from_min_size(egui::pos2(view.left() + 10.0, view.bottom() - side - 10.0), egui::vec2(side, side))
}

/// The plan of `level` fitted into `r`: (scale, offset) mapping feet to screen.
fn minimap_fit(scene: &Scene, level: usize, r: egui::Rect) -> Option<(f64, cadaddy_doc::geom::Vec2)> {
    let l = scene.levels.get(level)?;
    let (w, h) = (l.max.x - l.min.x, l.max.y - l.min.y);
    if !(w.is_finite() && h.is_finite()) || w <= 0.0 || h <= 0.0 {
        return None;
    }
    let pad = 8.0;
    let k = ((f64::from(r.width()) - 2.0 * pad) / w).min((f64::from(r.height()) - 2.0 * pad) / h);
    let off = cadaddy_doc::geom::Vec2 { x: f64::from(r.left()) + pad - l.min.x * k, y: f64::from(r.bottom()) - pad + l.min.y * k };
    Some((k, off))
}

/// The plan point under screen point `p` of the minimap.
fn minimap_to_plan(scene: &Scene, level: usize, r: egui::Rect, p: egui::Pos2) -> Option<cadaddy_doc::geom::Vec2> {
    let (k, off) = minimap_fit(scene, level, r)?;
    Some(cadaddy_doc::geom::Vec2 { x: (f64::from(p.x) - off.x) / k, y: (off.y - f64::from(p.y)) / k })
}

fn draw_minimap(painter: &egui::Painter, r: egui::Rect, scene: &Scene, w: &Walker) {
    painter.rect_filled(r, 4.0, egui::Color32::from_rgba_unmultiplied(20, 22, 26, 200));
    let Some((k, off)) = minimap_fit(scene, w.level, r) else { return };
    let to = |p: cadaddy_doc::geom::Vec2| egui::pos2((off.x + p.x * k) as f32, (off.y - p.y * k) as f32);
    let pen = egui::Stroke::new(1.0, egui::Color32::from_gray(210));
    for seg in scene.walls.iter().filter(|s| s.level == w.level).take(50_000) {
        painter.line_segment([to(seg.a), to(seg.b)], pen);
    }
    let door = egui::Stroke::new(1.5, egui::Color32::from_rgb(200, 150, 100));
    for d in scene.door_leaves.iter().filter(|d| d.level == w.level) {
        let (a, b) = d.line();
        painter.line_segment([to(a), to(b)], door);
    }
    let me = to(cadaddy_doc::geom::Vec2 { x: w.eye.x, y: w.eye.y });
    let ahead = egui::vec2(w.yaw.cos() as f32, -(w.yaw.sin() as f32)) * 12.0;
    painter.line_segment([me, me + ahead], egui::Stroke::new(2.0, egui::Color32::from_rgb(255, 200, 60)));
    painter.circle_filled(me, 4.0, egui::Color32::from_rgb(255, 200, 60));
}

/// The Layers list: walls, then each device type with its colour and count (right-click a type to
/// show only it). True when something changed.
fn layers_panel(walk: &mut Walk, ui: &mut egui::Ui) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.strong("Layers");
        if ui.small_button("All").clicked() {
            walk.hidden_types.clear();
            walk.walls_on = true;
            changed = true;
        }
        if ui.small_button("None").clicked() {
            walk.hidden_types = walk.types.iter().map(|t| t.0.clone()).collect();
            changed = true;
        }
    });
    changed |= ui.checkbox(&mut walk.walls_on, "Walls").changed();
    let mut only: Option<String> = None;
    egui::ScrollArea::vertical().id_salt("walk_layers").max_height(240.0).show(ui, |ui| {
        for (t, n, c) in &walk.types {
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                ui.painter().rect_filled(r, 2.0, egui::Color32::from_rgb(c[0], c[1], c[2]));
                let mut on = !walk.hidden_types.contains(t);
                let resp = ui.checkbox(&mut on, format!("{t} ({n})")).on_hover_text("Right-click: show only this");
                if resp.changed() {
                    if on {
                        walk.hidden_types.remove(t);
                    } else {
                        walk.hidden_types.insert(t.clone());
                    }
                    changed = true;
                }
                if resp.secondary_clicked() {
                    only = Some(t.clone());
                }
            });
        }
    });
    if let Some(t) = only {
        walk.hidden_types = walk.types.iter().map(|x| x.0.clone()).filter(|x| *x != t).collect();
        changed = true;
    }
    changed
}

/// A world point on screen (none behind the eye).
fn project(m: &cadaddy_walk3d::Mat4, rect: egui::Rect, p: cadaddy_walk3d::V3) -> Option<egui::Pos2> {
    let r = |i: usize| m.0.get(i).map(|row| row[0] * p.x + row[1] * p.y + row[2] * p.z + row[3]).unwrap_or(0.0);
    let (x, y, w) = (r(0), r(1), r(3));
    if w < 0.05 {
        return None;
    }
    let (nx, ny) = (x / w, y / w);
    Some(egui::pos2(rect.left() + ((nx + 1.0) / 2.0) as f32 * rect.width(), rect.top() + ((1.0 - ny) / 2.0) as f32 * rect.height()))
}

/// Dimension arrows over the 3D view: a yellow line with arrowheads at both ends and its length.
fn draw_dims(painter: &egui::Painter, rect: egui::Rect, m: &cadaddy_walk3d::Mat4, dims: &[cadaddy_walk3d::Dim]) {
    let col = egui::Color32::from_rgb(255, 214, 10);
    let stroke = egui::Stroke::new(2.0, col);
    let painter = painter.with_clip_rect(rect);
    for d in dims {
        let (Some(a), Some(b)) = (project(m, rect, d.a), project(m, rect, d.b)) else { continue };
        let v = b - a;
        if v.length() < 4.0 {
            continue;
        }
        painter.line_segment([a, b], stroke);
        let u = v.normalized();
        let n = egui::vec2(-u.y, u.x);
        for (tip, dir) in [(a, u), (b, -u)] {
            painter.line_segment([tip, tip + dir * 10.0 + n * 5.0], stroke);
            painter.line_segment([tip, tip + dir * 10.0 - n * 5.0], stroke);
        }
        let label = match d.kind {
            DimKind::Height => format!("{} AFF", ft_in(d.feet)),
            DimKind::ToWall => ft_in(d.feet),
        };
        let galley = painter.layout_no_wrap(label, egui::FontId::proportional(15.0), egui::Color32::BLACK);
        let mid = a + v / 2.0;
        let r = egui::Rect::from_center_size(mid, galley.size() + egui::vec2(8.0, 4.0));
        painter.rect_filled(r, 3.0, col);
        painter.galley(r.min + egui::vec2(4.0, 2.0), galley, egui::Color32::BLACK);
    }
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

    #[test]
    fn layers_hide_device_types_and_walls() {
        let rules = Rules::from_json(
            r#"{"levels":[{"name":"L1","elevation_ft":0,"height_ft":10}],
                "layers":[{"layer":"A-WALL","kind":"wall"},{"layer":"E-DEVC","kind":"device"}]}"#,
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let mut d = Drawing::default();
        let mut push = |h: u64, layer: &str, kind: EntityKind| {
            let mut e = cadaddy_doc::Entity::new(Handle(h), kind);
            e.common.layer = layer.into();
            d.model.push(e);
        };
        let v = |x: f64, y: f64| cadaddy_geom::Vec3 { x, y, z: 0.0 };
        push(1, "A-WALL", EntityKind::Line(cadaddy_doc::Line { a: v(0.0, 0.0), b: v(240.0, 0.0) }));
        // A second wall gives the level an area, so it gets a floor.
        push(5, "A-WALL", EntityKind::Line(cadaddy_doc::Line { a: v(240.0, 0.0), b: v(240.0, 120.0) }));
        for (h, t, x) in [(2u64, "light", 60.0), (3, "receptacle", 120.0), (4, "light", 180.0)] {
            let attr = serde_json::json!({"tag": "TYPE", "text": {"insert": v(x, 60.0), "height": 6.0, "value": t}});
            let ins = serde_json::json!({"block": "E-DEV", "insert": v(x, 60.0), "attribs": [attr]});
            push(h, "E-DEVC", EntityKind::Insert(serde_json::from_value(ins).unwrap_or_else(|e| panic!("{e}"))));
        }
        let scene = derive_set(&[&d], &rules);
        let b = Building { drawings: vec![Arc::new(d)], rules, rules_from: String::new(), scene, problems: Vec::new() };
        let tris_of = |s: &Scene, h: u64| s.owners.iter().filter(|o| o.handle == Handle(h)).count();
        let all = visible(&b, &BTreeSet::new(), true);
        assert!(tris_of(&all, 1) > 0 && tris_of(&all, 2) > 0 && tris_of(&all, 3) > 0);
        let no_lights = visible(&b, &BTreeSet::from(["light".to_string()]), true);
        assert_eq!((tris_of(&no_lights, 2), tris_of(&no_lights, 4)), (0, 0));
        assert_eq!(tris_of(&no_lights, 3), tris_of(&all, 3));
        let no_walls = visible(&b, &BTreeSet::new(), false);
        assert_eq!((tris_of(&no_walls, 1), tris_of(&no_walls, 5)), (0, 0));
        assert!(no_walls.edges.is_empty() && tris_of(&no_walls, 0) > 0, "floors stay");
        assert_eq!(no_walls.tris.len(), no_walls.owners.len() * 3);
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
        let mut e = cadaddy_doc::Entity::new(
            Handle(5),
            EntityKind::Line(cadaddy_doc::Line {
                a: cadaddy_geom::Vec3 { x: 0.0, y: 0.0, z: 0.0 },
                b: cadaddy_geom::Vec3 { x: 120.0, y: 0.0, z: 0.0 },
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

    #[test]
    fn a_drawing_with_a_building_spec_walks_by_it_and_its_stairs() {
        let d = Drawing {
            building_spec: Some(
            serde_json::json!({
                "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 12, "base": [0, 0]}, {"name": "L2", "elevation_ft": 12, "height_ft": 12, "region": [5000, 0, 6000, 100]}],
                "stairs": [{"name": "S1", "level": "L1", "runs": [{"from": [100, 22], "to": [298, 22], "treads": 18}], "width_in": 44}]
            })
            .to_string(),
        ),
            ..Drawing::default()
        };
        let b = Building::load(Arc::new(d), None).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(b.rules_from, "the drawing's building spec");
        assert_eq!(b.scene.levels.len(), 2);
        assert_eq!(b.scene.slopes.len(), 1);
        assert!(b.scene.parts.contains(&cadaddy_walk3d::Part::Stair), "the steps are drawn");
    }
}
