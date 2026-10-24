//! The field viewer (Q312): the building model on a tablet or phone on the job, offline.
//!
//! It opens a field package (`fieldview.export`, `cadaddy-walk3d`'s [`FieldPackage`]) and needs
//! nothing else: no drawing, no engine, no network. Pick a level and a room (search by room
//! number), look at it in plan, in 3D or standing in it (Walk), tap an element for what it is
//! (its BIM type: spec fields with their source and confidence), hide trades, measure, see the
//! room's open clashes and RFIs, and add a note or a verified point. The package and the notes
//! made are kept in the device's storage ([`FieldViewer::to_storage`]) so a reload without a
//! connection comes back as it was; Send Notes hands the notes not yet sent back as a
//! `cadaddy-fieldnotes` file that `fieldview.sync` merges into the drawing.
//!
//! The web app shows it with `?field` (and `?field=URL` fetches a package).
//!
//! A shared project page (Q350) shows it read-only ([`FieldViewer::read_only`]): no notes and no
//! Open; the model arrives in parts with a progress bar, a section box cuts it, elements are
//! found by tag, and the view in use travels as a `#view=` link ([`FieldViewer::view_link`]).

use std::collections::BTreeSet;

use cadaddy_walk3d::field::{Camera, FieldElement, FieldNote, FieldNotes, FieldPackage, IssueKind, Measured, NoteKind};
use cadaddy_walk3d::share::ViewLink;
use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Shape, Stroke, Vec2, pos2, vec2};

use crate::i18n::t;
use crate::theme::Tokens;

/// Storage key of the package (with its notes).
pub const STORAGE_KEY: &str = "cadaddy.fieldview";
/// Most boxes the 3D view draws (a phone's budget).
pub const MAX_3D_BOXES: usize = 6_000;
/// Width below which the details go in a bottom sheet (phone) instead of a side panel.
pub const PHONE_WIDTH: f32 = 760.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Plan,
    Model,
    Walk,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tool {
    #[default]
    Select,
    Measure,
    Note,
}

/// A note being written.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Draft {
    pub at: [f64; 3],
    pub kind: NoteKind,
    pub element: String,
    pub text: String,
}

#[derive(Debug)]
pub struct FieldViewer {
    pub pkg: Option<FieldPackage>,
    pub level: usize,
    pub room: Option<usize>,
    pub query: String,
    pub mode: Mode,
    pub tool: Tool,
    pub hidden: BTreeSet<String>,
    pub selected: Option<usize>,
    /// Measure points (two make a distance).
    pub measure: Vec<[f64; 3]>,
    pub draft: Option<Draft>,
    /// This device's tag in note ids.
    pub device: String,
    pub author: String,
    /// The last thing that happened, shown in the status line.
    pub status: String,
    /// The package or its notes changed since last stored.
    pub dirty: bool,
    /// Files waiting to be handed to the user (name, bytes): Send Notes.
    pub outbox: Vec<(String, Vec<u8>)>,
    /// Plan view: the point at the centre (feet) and pixels per foot; none fits on the next frame.
    pub plan: Option<([f64; 2], f32)>,
    pub yaw: f64,
    pub pitch: f64,
    pub dist: f64,
    pub walk_at: [f64; 2],
    pub heading: f64,
    /// Elements drawn by the last 3D frame (tests, and the status line).
    pub drawn_3d: usize,
    /// Open was pressed: the shell shows its file picker.
    pub open_requested: bool,
    /// A shared project page: read-only (no notes, no Open), views shared as links.
    pub read_only: bool,
    /// The section box: min and max corners, feet. What is wholly outside isn't shown or picked.
    pub section: Option<([f64; 3], [f64; 3])>,
    /// How far the orbit's target was panned off the room or level centre, feet.
    pub target_off: [f64; 3],
    /// The model loading (0..=1) while its parts arrive; none when it is all in.
    pub progress: Option<f32>,
    /// The server refused the model (401/403): the page says access is denied.
    pub denied: bool,
    /// A download of the model is offered (a page made with `--allow-download`).
    pub download_offered: bool,
    /// Download was pressed: the shell hands the model file over.
    pub download_requested: bool,
    /// A view link to hand out (`#view=` text): the shell copies it and shows it in the address bar.
    pub link_out: Option<String>,
}

/// One row of an element's properties: what, its value, and where it came from.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Prop {
    pub key: String,
    pub value: String,
    pub note: String,
}

impl Default for FieldViewer {
    fn default() -> Self {
        FieldViewer {
            pkg: None,
            level: 0,
            room: None,
            query: String::new(),
            mode: Mode::Plan,
            tool: Tool::Select,
            hidden: BTreeSet::new(),
            selected: None,
            measure: Vec::new(),
            draft: None,
            device: "field".into(),
            author: String::new(),
            status: String::new(),
            dirty: false,
            outbox: Vec::new(),
            plan: None,
            yaw: 235.0,
            pitch: 35.0,
            dist: 60.0,
            walk_at: [0.0, 0.0],
            heading: 90.0,
            drawn_3d: 0,
            open_requested: false,
            read_only: false,
            section: None,
            target_off: [0.0; 3],
            progress: None,
            denied: false,
            download_offered: false,
            download_requested: false,
            link_out: None,
        }
    }
}

fn rgb(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

impl FieldViewer {
    pub fn new(device: &str) -> FieldViewer {
        FieldViewer { device: device.into(), ..FieldViewer::default() }
    }

    /// Opens a package file.
    pub fn open(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.open_package(FieldPackage::from_json(bytes)?);
        Ok(())
    }

    /// Opens a package read already (a shared page's base, before its parts arrive).
    pub fn open_package(&mut self, mut pkg: FieldPackage) {
        pkg.assign_rooms();
        // Notes made here and not sent yet stay when the same project's package is replaced.
        if let Some(old) = &self.pkg
            && old.project == pkg.project
        {
            let mut keep = old.unsynced();
            keep.retain(|n| !pkg.notes.iter().any(|m| m.id == n.id));
            pkg.notes.extend(keep);
        }
        self.status = format!("{} — {} {}", pkg.project, pkg.elements.len(), t("elements"));
        self.pkg = Some(pkg);
        self.level = 0;
        self.room = None;
        self.selected = None;
        self.measure.clear();
        self.draft = None;
        self.plan = None;
        self.section = None;
        self.target_off = [0.0; 3];
        self.dirty = !self.read_only;
    }

    /// A part of a shared page's model arrived: its elements join the package (the view stays).
    pub fn add_elements(&mut self, elements: Vec<FieldElement>) {
        let Some(p) = self.pkg.as_mut() else { return };
        let room = cadaddy_walk3d::field::MAX_ELEMENTS.saturating_sub(p.elements.len());
        p.elements.extend(elements.into_iter().take(room));
        p.clean();
        p.assign_rooms();
        let counts = p.trade_counts();
        for k in counts.keys() {
            if !p.trades.iter().any(|t| &t.key == k) {
                p.trades.push(cadaddy_walk3d::field::trade_info(k));
            }
        }
        self.status = format!("{} — {} {}", p.project, p.elements.len(), t("elements"));
    }

    /// Whether `e` is in the section box (any part of it), or there is none.
    pub fn in_section(&self, e: &FieldElement) -> bool {
        self.section.is_none_or(|(a, b)| e.max.iter().zip(&a).all(|(hi, lo)| hi >= lo) && e.min.iter().zip(&b).all(|(lo, hi)| lo <= hi))
    }

    /// The section box around the level shown: its plan extent, its floor to the next.
    pub fn default_section(&self) -> ([f64; 3], [f64; 3]) {
        let (a, b) = self.level_box();
        let z = self.floor_z();
        let h = self.pkg.as_ref().and_then(|p| p.levels.get(self.level)).map_or(10.0, |l| l.height_ft.max(1.0));
        ([a[0] - 2.0, a[1] - 2.0, z], [b[0] + 2.0, b[1] + 2.0, z + h])
    }

    /// Elements whose tag (BIM mark), id or kind matches `query`: marks and ids exactly first,
    /// then marks and ids starting with it, then kinds holding it. At most 12.
    pub fn search_elements(&self, query: &str) -> Vec<usize> {
        let q = query.trim().to_lowercase();
        let Some(p) = &self.pkg else { return Vec::new() };
        if q.chars().count() < 2 {
            return Vec::new();
        }
        let mut hits: Vec<(u8, usize)> = p
            .elements
            .iter()
            .enumerate()
            .filter(|(_, e)| !self.hidden.contains(&e.trade))
            .filter_map(|(i, e)| {
                let (mark, id) = (e.mark.to_lowercase(), e.id.to_lowercase());
                let rank = if mark == q || id == q {
                    0
                } else if mark.starts_with(&q) || id.starts_with(&q) {
                    1
                } else if e.kind.to_lowercase().contains(&q) {
                    2
                } else {
                    return None;
                };
                Some((rank, i))
            })
            .take(5_000)
            .collect();
        hits.sort_unstable();
        hits.into_iter().map(|(_, i)| i).take(12).collect()
    }

    /// Goes to an element: its level and room, selected.
    pub fn go_to_element(&mut self, i: usize) {
        let Some(p) = &self.pkg else { return };
        let Some(e) = p.elements.get(i) else { return };
        let room =
            p.rooms.iter().position(|r| !e.room.is_empty() && r.number.eq_ignore_ascii_case(&e.room) && r.level.eq_ignore_ascii_case(&e.level));
        let level = p.levels.iter().position(|l| l.name.eq_ignore_ascii_case(&e.level));
        let centre = [(e.min[0] + e.max[0]) * 0.5, (e.min[1] + e.max[1]) * 0.5];
        let inside = self.in_section(e);
        match room {
            Some(r) => self.select_room(r),
            None => {
                if let Some(l) = level {
                    self.level = l;
                }
                self.room = None;
                self.walk_at = centre;
                self.plan = Some((centre, self.plan.map_or(20.0, |(_, s)| s.max(8.0))));
                self.query.clear();
            }
        }
        if !inside {
            self.section = None;
        }
        self.selected = Some(i);
    }

    /// The selected element's properties: size, height, room, source, status, its type's spec
    /// fields (with their source and confidence) and its own properties (with their set).
    pub fn properties(&self) -> Vec<Prop> {
        let Some(p) = &self.pkg else { return Vec::new() };
        let Some(e) = self.selected.and_then(|i| p.elements.get(i)) else { return Vec::new() };
        let row = |k: &str, v: String, note: String| Prop { key: k.to_string(), value: v, note };
        let mut out = Vec::new();
        if let Some(s) = e.size_in {
            let size = if s[1] > 0.0 { format!("{}x{}\"", s[0], s[1]) } else { format!("{}\"", s[0]) };
            out.push(row(t("Size"), size, String::new()));
        }
        let z = p.levels.iter().find(|l| l.name.eq_ignore_ascii_case(&e.level)).map_or(0.0, |l| l.elevation_ft);
        out.push(row(
            t("Bottom / top AFF"),
            format!("{} / {}", cadaddy_walk3d::feet(e.min[2] - z), cadaddy_walk3d::feet(e.max[2] - z)),
            String::new(),
        ));
        if !e.room.is_empty() {
            out.push(row(t("Room"), e.room.clone(), String::new()));
        }
        if !e.source.is_empty() {
            out.push(row(t("From"), e.source.clone(), String::new()));
        }
        if !e.status.is_empty() {
            out.push(row(t("Status"), e.status.clone(), String::new()));
        }
        match p.type_of(e) {
            Some(ty) => {
                out.push(row(t("Type"), format!("{} {}", ty.mark, ty.category).trim().to_string(), String::new()));
                for f in &ty.fields {
                    let note = match (f.source.is_empty(), f.confidence.is_empty()) {
                        (false, false) => format!("({}, {})", f.source, t(&f.confidence)),
                        (false, true) => format!("({})", f.source),
                        (true, false) => format!("({})", t(&f.confidence)),
                        (true, true) => String::new(),
                    };
                    out.push(row(&f.key, f.value.clone(), note));
                }
            }
            None if !e.mark.is_empty() => out.push(row(t("Type"), e.mark.clone(), String::new())),
            None => out.push(row(t("Type"), t("not typed").to_string(), String::new())),
        }
        for f in &e.props {
            out.push(row(&f.key, f.value.clone(), if f.source.is_empty() { String::new() } else { format!("({})", f.source) }));
        }
        out
    }

    /// The view in use as a link ([`ViewLink`]).
    pub fn view_link(&self) -> ViewLink {
        let p = self.pkg.as_ref();
        ViewLink {
            level: self.level,
            mode: match self.mode {
                Mode::Plan => "plan",
                Mode::Model => "3d",
                Mode::Walk => "walk",
            }
            .into(),
            orbit: (self.mode == Mode::Model).then_some([self.yaw, self.pitch, self.dist]),
            target: (self.mode == Mode::Model && self.target_off != [0.0; 3]).then_some(self.target_off),
            walk: (self.mode == Mode::Walk).then_some([self.walk_at[0], self.walk_at[1], self.heading]),
            plan: self.plan.filter(|_| self.mode == Mode::Plan).map(|(c, s)| [c[0], c[1], f64::from(s)]),
            hidden: self.hidden.iter().cloned().collect(),
            section: self.section.map(|(a, b)| [a[0], a[1], a[2], b[0], b[1], b[2]]),
            room: self.room.and_then(|i| p?.rooms.get(i)).map(|r| r.number.clone()).unwrap_or_default(),
            element: self.selected.and_then(|i| p?.elements.get(i)).map(|e| e.id.clone()).unwrap_or_default(),
        }
    }

    /// Shows a view from a link; what the model doesn't have (a level, room or element) is skipped.
    pub fn apply_view(&mut self, v: &ViewLink) {
        let Some(p) = &self.pkg else { return };
        let level = v.level.min(p.levels.len().saturating_sub(1));
        let level_name = p.levels.get(level).map(|l| l.name.clone()).unwrap_or_default();
        let room = if v.room.is_empty() {
            None
        } else {
            p.rooms
                .iter()
                .position(|r| r.number.eq_ignore_ascii_case(&v.room) && r.level.eq_ignore_ascii_case(&level_name))
                .or_else(|| p.rooms.iter().position(|r| r.number.eq_ignore_ascii_case(&v.room)))
        };
        let element = if v.element.is_empty() { None } else { p.elements.iter().position(|e| e.id == v.element) };
        match room {
            Some(r) => self.select_room(r),
            None => {
                self.level = level;
                self.room = None;
                self.plan = None;
            }
        }
        self.mode = match v.mode.as_str() {
            "3d" => Mode::Model,
            "walk" => Mode::Walk,
            _ => Mode::Plan,
        };
        if let Some([y, pi, d]) = v.orbit {
            (self.yaw, self.pitch, self.dist) = (y, pi.clamp(-10.0, 89.0), d.clamp(3.0, 2_000.0));
        }
        self.target_off = v.target.unwrap_or([0.0; 3]);
        if let Some([x, y, h]) = v.walk {
            (self.walk_at, self.heading) = ([x, y], h);
        }
        if let Some([x, y, sc]) = v.plan {
            self.plan = Some(([x, y], (sc as f32).clamp(0.5, 400.0)));
        }
        self.hidden = v.hidden.iter().cloned().collect();
        self.section = v.section.map(|[a, b, c, d, e, f]| ([a.min(d), b.min(e), c.min(f)], [a.max(d), b.max(e), c.max(f)]));
        self.selected = element;
    }

    /// What to keep in the device's storage: the package and its notes.
    pub fn to_storage(&self) -> Option<String> {
        self.pkg.as_ref().and_then(|p| p.to_json().ok())
    }

    /// Comes back from the device's storage (no network needed).
    pub fn from_storage(&mut self, text: &str) -> Result<(), String> {
        self.open(text.as_bytes())?;
        self.dirty = false;
        Ok(())
    }

    pub fn level_name(&self) -> String {
        self.pkg.as_ref().and_then(|p| p.levels.get(self.level)).map(|l| l.name.clone()).unwrap_or_default()
    }

    fn floor_z(&self) -> f64 {
        self.pkg.as_ref().and_then(|p| p.levels.get(self.level)).map_or(0.0, |l| l.elevation_ft)
    }

    /// Goes to a room: its level, centred in plan, standing in it in Walk.
    pub fn select_room(&mut self, i: usize) {
        let Some(p) = &self.pkg else { return };
        let Some(r) = p.rooms.get(i) else { return };
        if let Some(l) = p.levels.iter().position(|l| l.name.eq_ignore_ascii_case(&r.level)) {
            self.level = l;
        }
        let (min, max) = room_box(&r.outline).unwrap_or(([r.at[0] - 10.0, r.at[1] - 10.0], [r.at[0] + 10.0, r.at[1] + 10.0]));
        self.walk_at = r.at;
        self.dist = ((max[0] - min[0]).max(max[1] - min[1]) * 2.2).clamp(15.0, 400.0);
        self.room = Some(i);
        self.selected = None;
        self.query.clear();
        self.plan = None;
        self.status = format!("{} {} {}", t("Room"), r.number, r.name);
    }

    /// The elements a view shows: the room's (and what is near it) when a room is picked, else
    /// the level's; hidden trades left out; capped.
    pub fn visible(&self) -> Vec<usize> {
        let Some(p) = &self.pkg else { return Vec::new() };
        let level = self.level_name();
        let near = self
            .room
            .and_then(|i| p.rooms.get(i))
            .and_then(|r| room_box(&r.outline))
            .map(|(a, b)| ([a[0] - 4.0, a[1] - 4.0], [b[0] + 4.0, b[1] + 4.0]));
        p.elements
            .iter()
            .enumerate()
            .filter(|(_, e)| !self.hidden.contains(&e.trade) && e.level.eq_ignore_ascii_case(&level) && self.in_section(e))
            .filter(|(_, e)| near.is_none_or(|(a, b)| e.max[0] >= a[0] && e.min[0] <= b[0] && e.max[1] >= a[1] && e.min[1] <= b[1]))
            .map(|(i, _)| i)
            .take(MAX_3D_BOXES)
            .collect()
    }

    /// A tap in plan at `p` (feet) within `tol` feet, by the tool in use.
    pub fn tap_plan(&mut self, p: [f64; 2], tol: f64) {
        let z = self.floor_z();
        let level = self.level_name();
        let Some(pkg) = &self.pkg else { return };
        match self.tool {
            Tool::Select => {
                self.selected = pkg.pick_plan(&level, p, tol, &self.hidden).filter(|i| pkg.elements.get(*i).is_some_and(|e| self.in_section(e)));
                if self.selected.is_none()
                    && let Some(r) = pkg.room_at(&level, p)
                {
                    self.select_room(r);
                }
            }
            Tool::Measure => self.measure_point([p[0], p[1], z]),
            Tool::Note if !self.read_only => self.start_note([p[0], p[1], z]),
            Tool::Note => {}
        }
    }

    /// A tap in 3D: the ray from the camera through it.
    pub fn tap_ray(&mut self, origin: [f64; 3], dir: [f64; 3]) {
        let level = self.level_name();
        let Some(pkg) = &self.pkg else { return };
        let hit = pkg.pick_ray(origin, dir, Some(&level), &self.hidden).filter(|(i, _)| pkg.elements.get(*i).is_some_and(|e| self.in_section(e)));
        let at = hit.map(|(_, t)| [origin[0] + dir[0] * t, origin[1] + dir[1] * t, origin[2] + dir[2] * t]);
        match (self.tool, hit, at) {
            (Tool::Select, h, _) => self.selected = h.map(|(i, _)| i),
            (Tool::Measure, _, Some(a)) => self.measure_point(a),
            (Tool::Note, h, Some(a)) if !self.read_only => {
                self.selected = h.map(|(i, _)| i);
                self.start_note(a);
            }
            _ => {}
        }
    }

    fn measure_point(&mut self, a: [f64; 3]) {
        if self.measure.len() >= 2 {
            self.measure.clear();
        }
        self.measure.push(a);
        if let [x, y] = self.measure.as_slice() {
            self.status = format!("{} {}", t("Distance"), Measured::between(*x, *y).text());
        }
    }

    fn start_note(&mut self, at: [f64; 3]) {
        let element = self.selected.and_then(|i| self.pkg.as_ref()?.elements.get(i)).map(|e| e.id.clone()).unwrap_or_default();
        self.draft = Some(Draft { at, kind: NoteKind::Note, element, text: String::new() });
    }

    /// Keeps the note being written (empty text keeps nothing).
    pub fn save_draft(&mut self, when: &str) -> Option<String> {
        let d = self.draft.take()?;
        if d.text.trim().is_empty() {
            return None;
        }
        let level = self.level_name();
        let (device, author) = (self.device.clone(), self.author.clone());
        let pkg = self.pkg.as_mut()?;
        let id = pkg.add_note(
            &device,
            FieldNote { kind: d.kind, level, at: d.at, element: d.element, text: d.text, author, when: when.into(), ..FieldNote::default() },
        )?;
        self.dirty = true;
        self.status = format!("{} {id}", t("Note kept"));
        Some(id)
    }

    /// Send Notes: the notes not sent yet as a notes file in the outbox, then marked sent.
    pub fn send_notes(&mut self) -> usize {
        let Some(pkg) = self.pkg.as_mut() else { return 0 };
        let notes = pkg.unsynced();
        if notes.is_empty() {
            self.status = t("No new notes to send").into();
            return 0;
        }
        let n = notes.len();
        let ids: Vec<String> = notes.iter().map(|x| x.id.clone()).collect();
        let file = FieldNotes::new(&pkg.project, notes);
        let Ok(bytes) = serde_json::to_vec_pretty(&file) else { return 0 };
        let name: String = pkg.project.chars().map(|c| if c.is_alphanumeric() || c == '-' { c } else { '_' }).take(60).collect();
        self.outbox.push((format!("{name}-fieldnotes-{}.json", self.device), bytes));
        pkg.mark_synced(&ids);
        self.dirty = true;
        self.status = format!("{n} {}", t("notes ready to sync"));
        n
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let tk = Tokens::get();
        egui::Panel::top("fv_top").frame(egui::Frame::NONE.fill(tk.chrome).inner_margin(6.0)).show(ui, |ui| self.top_bar(ui));
        if self.pkg.is_none() {
            egui::CentralPanel::default().frame(egui::Frame::NONE.fill(tk.canvas)).show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(ui.available_height() * 0.4);
                    if self.denied {
                        ui.label(egui::RichText::new(t("Access denied")).size(24.0).strong().color(tk.text));
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new(t("This project page needs a passcode. Ask whoever shared the link for access.")).color(tk.text_dim),
                        );
                    } else if let Some(f) = self.progress {
                        ui.label(egui::RichText::new(t("Loading the model…")).size(18.0).color(tk.text_dim));
                        ui.add(egui::ProgressBar::new(f).show_percentage().desired_width(320.0));
                    } else if self.read_only {
                        let text = if self.status.is_empty() { t("Loading the model…") } else { self.status.as_str() };
                        ui.label(egui::RichText::new(text).size(18.0).color(tk.text_dim));
                    } else {
                        ui.label(
                            egui::RichText::new(t("Open a field package (FIELDVIEW.EXPORT in CADaddy), or drop one here"))
                                .size(18.0)
                                .color(tk.text_dim),
                        );
                    }
                });
            });
            return;
        }
        let phone = ui.available_width() < PHONE_WIDTH;
        if phone {
            egui::Panel::bottom("fv_sheet").exact_size(ui.available_height() * 0.38).frame(egui::Frame::NONE.fill(tk.panel).inner_margin(8.0)).show(
                ui,
                |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| self.details(ui));
                },
            );
        } else {
            egui::Panel::right("fv_side").exact_size(320.0).resizable(false).frame(egui::Frame::NONE.fill(tk.panel).inner_margin(8.0)).show(
                ui,
                |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| self.details(ui));
                },
            );
        }
        egui::CentralPanel::default().frame(egui::Frame::NONE.fill(tk.canvas)).show(ui, |ui| self.canvas(ui));
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let tk = Tokens::get();
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().button_padding = vec2(12.0, 8.0);
            if !self.read_only && ui.button(t("Open")).clicked() {
                self.open_requested = true;
            }
            let Some(p) = &self.pkg else {
                ui.label(egui::RichText::new(t("Field Viewer")).strong().color(tk.text));
                return;
            };
            ui.label(egui::RichText::new(&p.project).strong().color(tk.text));
            let levels: Vec<String> = p.levels.iter().map(|l| l.name.clone()).collect();
            let mut level = self.level;
            egui::ComboBox::from_id_salt("fv_level").selected_text(levels.get(level).cloned().unwrap_or_default()).show_ui(ui, |ui| {
                for (i, n) in levels.iter().enumerate() {
                    ui.selectable_value(&mut level, i, n);
                }
            });
            if level != self.level {
                self.level = level;
                self.room = None;
                self.selected = None;
                self.plan = None;
            }
            let hint = if self.read_only { t("Room or tag") } else { t("Room number") };
            ui.add(egui::TextEdit::singleline(&mut self.query).hint_text(hint).desired_width(130.0));
            for (m, label) in [(Mode::Plan, "Plan"), (Mode::Model, "3D"), (Mode::Walk, "Walk")] {
                if ui.selectable_label(self.mode == m, t(label)).clicked() {
                    self.mode = m;
                }
            }
            ui.separator();
            for (tool, label) in [(Tool::Select, "Select"), (Tool::Measure, "Measure"), (Tool::Note, "Note")] {
                if self.read_only && tool == Tool::Note {
                    continue;
                }
                if ui.selectable_label(self.tool == tool, t(label)).clicked() {
                    self.tool = tool;
                    self.measure.clear();
                }
            }
            ui.separator();
            if self.read_only {
                if ui.button(t("Copy View Link")).clicked() {
                    self.link_out = Some(self.view_link().encode());
                    self.status = t("View link copied").into();
                }
                if self.download_offered && ui.button(t("Download Model")).clicked() {
                    self.download_requested = true;
                }
            } else {
                let unsent = self.pkg.as_ref().map_or(0, |p| p.unsynced().len());
                if ui.add_enabled(unsent > 0, egui::Button::new(format!("{} ({unsent})", t("Send Notes")))).clicked() {
                    self.send_notes();
                }
            }
            if ui.button(t("Fit")).clicked() {
                self.plan = None;
                self.room = None;
                self.target_off = [0.0; 3];
            }
            if let Some(f) = self.progress.filter(|_| self.pkg.is_some()) {
                ui.add(egui::ProgressBar::new(f).show_percentage().desired_width(120.0));
            }
        });
        // Search results under the bar.
        if let Some(p) = &self.pkg
            && !self.query.trim().is_empty()
        {
            let hits = p.search_rooms(&self.query);
            let tagged = if hits.len() < 12 { self.search_elements(&self.query) } else { Vec::new() };
            let mut go = None;
            let mut go_el = None;
            ui.horizontal_wrapped(|ui| {
                if hits.is_empty() && tagged.is_empty() {
                    ui.label(egui::RichText::new(t("No room matches")).color(tk.text_dim));
                }
                for i in tagged.iter().take(12usize.saturating_sub(hits.len())) {
                    if let Some(e) = p.elements.get(*i) {
                        let tag = if e.mark.is_empty() { e.kind.clone() } else { format!("{} {}", e.mark, e.kind) };
                        let at = if e.room.is_empty() { e.level.clone() } else { format!("{} {}", e.level, e.room) };
                        if ui.button(format!("{tag} · {at}")).clicked() {
                            go_el = Some(*i);
                        }
                    }
                }
                for (i, _) in hits.iter().take(12) {
                    if let Some(r) = p.rooms.get(*i)
                        && ui.button(format!("{} {} · {}", r.number, r.name, r.level)).clicked()
                    {
                        go = Some(*i);
                    }
                }
            });
            if let Some(i) = go {
                self.select_room(i);
            }
            if let Some(i) = go_el {
                self.go_to_element(i);
                self.query.clear();
            }
        }
    }

    fn details(&mut self, ui: &mut egui::Ui) {
        let tk = Tokens::get();
        let Some(p) = self.pkg.as_ref() else { return };
        // Trades.
        ui.label(egui::RichText::new(t("Trades")).strong().color(tk.text));
        let counts = p.trade_counts();
        let mut toggle = None;
        for tr in &p.trades {
            let on = !self.hidden.contains(&tr.key);
            ui.horizontal(|ui| {
                let (sw, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                ui.painter().rect_filled(sw, 2.0, if on { rgb(tr.color) } else { tk.text_faint.gamma_multiply(0.4) });
                let mut shown = on;
                let label = format!("{} ({})", t(&tr.label), counts.get(&tr.key).copied().unwrap_or(0));
                if ui.checkbox(&mut shown, label).changed() {
                    toggle = Some(tr.key.clone());
                }
            });
        }
        if let Some(k) = toggle
            && !self.hidden.remove(&k)
        {
            self.hidden.insert(k);
            if self.selected.and_then(|i| p.elements.get(i)).is_some_and(|e| self.hidden.contains(&e.trade)) {
                self.selected = None;
            }
        }
        ui.separator();
        // The section box.
        let mut on = self.section.is_some();
        if ui.checkbox(&mut on, t("Section box")).changed() {
            self.section = on.then(|| self.default_section());
        }
        if let Some((a, b)) = &mut self.section {
            egui::Grid::new("fv_section").num_columns(3).show(ui, |ui| {
                for (k, axis) in ["X", "Y", "Z"].iter().enumerate() {
                    ui.label(egui::RichText::new(*axis).color(tk.text_dim));
                    if let (Some(lo), Some(hi)) = (a.get_mut(k), b.get_mut(k)) {
                        ui.add(egui::DragValue::new(lo).speed(0.25).suffix(" ft"));
                        ui.add(egui::DragValue::new(hi).speed(0.25).suffix(" ft"));
                        if *lo > *hi {
                            std::mem::swap(lo, hi);
                        }
                    }
                    ui.end_row();
                }
            });
        }
        ui.separator();
        // The measure.
        if let [a, b] = self.measure.as_slice() {
            ui.label(egui::RichText::new(format!("{}: {}", t("Distance"), Measured::between(*a, *b).text())).color(tk.accent));
            ui.separator();
        } else if self.tool == Tool::Measure {
            ui.label(egui::RichText::new(t("Tap two points to measure")).color(tk.text_dim));
            ui.separator();
        }
        // The note being written.
        let mut keep = false;
        let mut drop = false;
        if let Some(d) = &mut self.draft {
            ui.label(egui::RichText::new(t("New note")).strong().color(tk.text));
            ui.horizontal(|ui| {
                ui.radio_value(&mut d.kind, NoteKind::Note, t("Note"));
                ui.radio_value(&mut d.kind, NoteKind::VerifiedPoint, t("Verified point"));
            });
            ui.add(egui::TextEdit::multiline(&mut d.text).desired_rows(3).hint_text(t("What is there, as built")));
            ui.horizontal(|ui| {
                keep = ui.button(t("Keep")).clicked();
                drop = ui.button(t("Cancel")).clicked();
            });
            ui.separator();
        }
        if keep {
            let when = now_text(ui.ctx());
            self.save_draft(&when);
        } else if drop {
            self.draft = None;
        }
        let Some(p) = self.pkg.as_ref() else { return };
        // The tapped element: what it is, and what set it.
        if let Some(e) = self.selected.and_then(|i| p.elements.get(i)) {
            let trade = p.trade(&e.trade);
            ui.label(egui::RichText::new(format!("{} {}", t(&trade.label), e.kind)).strong().color(rgb(trade.color)));
            for r in self.properties() {
                ui.horizontal_wrapped(|ui| {
                    ui.label(egui::RichText::new(&r.key).color(tk.text_dim));
                    ui.label(egui::RichText::new(&r.value).color(tk.text));
                    if !r.note.is_empty() {
                        ui.label(egui::RichText::new(&r.note).color(tk.text_faint).small());
                    }
                });
            }
            ui.separator();
        }
        // The room: its unit, open clashes and RFIs, notes.
        if let Some(r) = self.room.and_then(|i| p.rooms.get(i)) {
            ui.label(egui::RichText::new(format!("{} {} {}", t("Room"), r.number, r.name)).strong().color(tk.text));
            if !r.unit.is_empty() {
                ui.label(egui::RichText::new(format!("{}: {}", t("Unit"), r.unit)).color(tk.text_dim));
            }
            let open: Vec<_> = p.open_issues(r).collect();
            ui.label(egui::RichText::new(format!("{} ({})", t("Open clashes and RFIs"), open.len())).color(tk.text));
            let mut go = None;
            for i in open.iter().take(50) {
                let tag = match i.kind {
                    IssueKind::Clash => t("Clash"),
                    IssueKind::Rfi => t("RFI"),
                };
                if ui.selectable_label(false, format!("{tag} {} · {} · {}", i.id, i.title, i.status)).clicked() {
                    go = i.elements.first().and_then(|id| p.elements.iter().position(|e| &e.id == id));
                }
            }
            let notes: Vec<&FieldNote> =
                p.notes.iter().filter(|n| n.room.eq_ignore_ascii_case(&r.number) && n.level.eq_ignore_ascii_case(&r.level)).collect();
            ui.label(egui::RichText::new(format!("{} ({})", t("Notes"), notes.len())).color(tk.text));
            for n in notes.iter().take(50) {
                let kind = if n.kind == NoteKind::VerifiedPoint { t("Verified") } else { t("Note") };
                let sent = if n.synced { "" } else { " •" };
                ui.label(egui::RichText::new(format!("{kind} {}{sent}: {}", n.id, n.text)).color(tk.text_dim));
            }
            if go.is_some() {
                self.selected = go;
            }
        } else {
            ui.label(egui::RichText::new(t("Search a room number, or tap a room")).color(tk.text_dim));
        }
        if !self.status.is_empty() {
            ui.separator();
            ui.label(egui::RichText::new(&self.status).color(tk.text_faint).small());
        }
    }

    fn canvas(&mut self, ui: &mut egui::Ui) {
        let (resp, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
        let rect = resp.rect;
        let zoom = ui.input(|i| i.zoom_delta());
        let scroll = ui.input(|i| i.smooth_scroll_delta);
        let pan_drag = resp.dragged_by(egui::PointerButton::Secondary)
            || resp.dragged_by(egui::PointerButton::Middle)
            || (resp.dragged_by(egui::PointerButton::Primary) && ui.input(|i| i.modifiers.shift));
        let two_finger = ui.input(|i| i.multi_touch()).map(|m| m.translation_delta).filter(|_| resp.hovered());
        let drag = if pan_drag { Vec2::ZERO } else { resp.drag_delta() };
        let pan_by = if pan_drag { resp.drag_delta() } else { Vec2::ZERO } + two_finger.unwrap_or(Vec2::ZERO);
        self.keys(ui);
        match self.mode {
            Mode::Plan => {
                if self.plan.is_none() {
                    self.plan = Some(self.fit_plan(rect));
                }
                let Some((mut c, mut s)) = self.plan else { return };
                if resp.hovered() && (zoom - 1.0).abs() > 1e-4 {
                    s = (s * zoom).clamp(0.5, 400.0);
                }
                let pan = drag + pan_by + if resp.hovered() { scroll } else { Vec2::ZERO };
                c[0] -= f64::from(pan.x / s);
                c[1] += f64::from(pan.y / s);
                self.plan = Some((c, s));
                let tap = resp.clicked().then(|| resp.interact_pointer_pos()).flatten();
                if let Some(at) = tap {
                    let p = [c[0] + f64::from((at.x - rect.center().x) / s), c[1] - f64::from((at.y - rect.center().y) / s)];
                    self.tap_plan(p, f64::from(14.0 / s));
                }
                self.draw_plan(&painter, rect, c, s);
            }
            Mode::Model | Mode::Walk => {
                if self.mode == Mode::Model {
                    if pan_by != Vec2::ZERO {
                        self.pan_orbit(pan_by);
                    }
                    self.yaw -= f64::from(drag.x) * 0.4;
                    self.pitch = (self.pitch + f64::from(drag.y) * 0.3).clamp(-10.0, 89.0);
                    if resp.hovered() && (zoom - 1.0).abs() > 1e-4 {
                        self.dist = (self.dist / f64::from(zoom)).clamp(3.0, 2_000.0);
                    }
                } else {
                    self.heading -= f64::from(drag.x) * 0.3;
                    let step = f64::from(scroll.y) * 0.05;
                    let h = self.heading.to_radians();
                    self.walk_at = [self.walk_at[0] + h.cos() * step, self.walk_at[1] + h.sin() * step];
                }
                let cam = self.camera();
                if let Some(at) = resp.clicked().then(|| resp.interact_pointer_pos()).flatten() {
                    let (o, d) =
                        cam.ray(f64::from(at.x - rect.min.x), f64::from(at.y - rect.min.y), f64::from(rect.width()), f64::from(rect.height()));
                    self.tap_ray(o, d);
                }
                self.draw_3d(&painter, rect, &cam);
                if self.mode == Mode::Walk {
                    self.walk_pad(ui, rect);
                }
            }
        }
    }

    /// The 3D camera: orbiting the room (or level), or standing in it.
    pub fn camera(&self) -> Camera {
        let z = self.floor_z();
        match self.mode {
            Mode::Walk => Camera::walk(self.walk_at, z, self.heading),
            _ => {
                let centre = match self.room.and_then(|i| self.pkg.as_ref()?.rooms.get(i)) {
                    Some(r) => [r.at[0], r.at[1], z + 4.0],
                    None => {
                        let (a, b) = self.level_box();
                        [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5, z + 4.0]
                    }
                };
                let o = self.target_off;
                let target = [centre[0] + o[0], centre[1] + o[1], centre[2] + o[2]];
                let dist = if self.room.is_some() {
                    self.dist
                } else {
                    let (a, b) = self.level_box();
                    ((b[0] - a[0]).max(b[1] - a[1]) * 1.2).max(self.dist)
                };
                Camera::orbit(target, dist, self.yaw, self.pitch)
            }
        }
    }

    fn level_box(&self) -> ([f64; 2], [f64; 2]) {
        let level = self.level_name();
        let mut lo = [f64::INFINITY; 2];
        let mut hi = [f64::NEG_INFINITY; 2];
        if let Some(p) = &self.pkg {
            for e in p.elements.iter().filter(|e| e.level.eq_ignore_ascii_case(&level)) {
                lo = [lo[0].min(e.min[0]), lo[1].min(e.min[1])];
                hi = [hi[0].max(e.max[0]), hi[1].max(e.max[1])];
            }
            for r in p.rooms.iter().filter(|r| r.level.eq_ignore_ascii_case(&level)) {
                for q in r.outline.iter().chain(std::iter::once(&r.at)) {
                    lo = [lo[0].min(q[0]), lo[1].min(q[1])];
                    hi = [hi[0].max(q[0]), hi[1].max(q[1])];
                }
            }
        }
        if lo[0].is_finite() && hi[0].is_finite() { (lo, hi) } else { ([-20.0, -20.0], [20.0, 20.0]) }
    }

    fn fit_plan(&self, rect: Rect) -> ([f64; 2], f32) {
        let (a, b) = match self.room.and_then(|i| self.pkg.as_ref()?.rooms.get(i)).and_then(|r| room_box(&r.outline)) {
            Some((a, b)) => ([a[0] - 6.0, a[1] - 6.0], [b[0] + 6.0, b[1] + 6.0]),
            None => self.level_box(),
        };
        let (w, h) = ((b[0] - a[0]).max(1.0), (b[1] - a[1]).max(1.0));
        let s = (f64::from(rect.width()) / w).min(f64::from(rect.height()) / h) * 0.92;
        ([(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5], (s as f32).clamp(0.5, 400.0))
    }

    fn draw_plan(&mut self, painter: &egui::Painter, rect: Rect, c: [f64; 2], s: f32) {
        let tk = Tokens::get();
        let Some(p) = &self.pkg else { return };
        let level = self.level_name();
        let to = |q: [f64; 2]| -> Pos2 { pos2(rect.center().x + ((q[0] - c[0]) as f32) * s, rect.center().y - ((q[1] - c[1]) as f32) * s) };
        // Rooms.
        for (i, r) in p.rooms.iter().enumerate().filter(|(_, r)| r.level.eq_ignore_ascii_case(&level)) {
            let pts: Vec<Pos2> = r.outline.iter().map(|q| to(*q)).collect();
            let picked = self.room == Some(i);
            if pts.len() >= 3 {
                let fill = if picked { tk.accent.gamma_multiply(0.18) } else { Color32::TRANSPARENT };
                painter.add(Shape::convex_polygon(
                    pts.clone(),
                    fill,
                    Stroke::new(if picked { 2.0 } else { 1.0 }, if picked { tk.accent } else { tk.grid_major }),
                ));
            }
            if s > 4.0 {
                painter.text(to(r.at), Align2::CENTER_CENTER, &r.number, FontId::proportional(12.0), tk.text_dim);
            }
        }
        // Elements: boxes, then runs over them.
        let vis = self.visible_level(&level);
        for &i in &vis {
            let Some(e) = p.elements.get(i) else { continue };
            if e.path.len() >= 2 {
                continue;
            }
            let col = rgb(p.trade(&e.trade).color);
            let r = Rect::from_two_pos(to([e.min[0], e.min[1]]), to([e.max[0], e.max[1]]));
            let r = if r.width() < 3.0 || r.height() < 3.0 { Rect::from_center_size(r.center(), r.size().max(vec2(4.0, 4.0))) } else { r };
            painter.rect_filled(r, 0.0, col.gamma_multiply(if e.trade == "arch" { 0.8 } else { 0.9 }));
        }
        for &i in &vis {
            let Some(e) = p.elements.get(i) else { continue };
            if e.path.len() < 2 {
                continue;
            }
            let col = rgb(p.trade(&e.trade).color);
            let w = (e.size_in.map_or(0.0, |z| z[0]) as f32 / 12.0 * s).max(2.0);
            let pts: Vec<Pos2> = e.path.iter().map(|q| to([q[0], q[1]])).collect();
            painter.add(Shape::line(pts, Stroke::new(w, col.gamma_multiply(0.85))));
        }
        if let Some(e) = self.selected.and_then(|i| p.elements.get(i)) {
            let r = Rect::from_two_pos(to([e.min[0], e.min[1]]), to([e.max[0], e.max[1]])).expand(3.0);
            painter.rect_stroke(r, 0.0, Stroke::new(2.5, tk.selection), egui::StrokeKind::Outside);
        }
        // Open issues and notes.
        for i in p.issues.iter().filter(|i| i.is_open() && i.level.eq_ignore_ascii_case(&level)) {
            let at = to([i.at[0], i.at[1]]);
            let col = if i.kind == IssueKind::Clash { Color32::from_rgb(230, 60, 50) } else { Color32::from_rgb(240, 150, 30) };
            painter.circle(at, 7.0, col, Stroke::new(1.5, Color32::WHITE));
            painter.text(at, Align2::CENTER_CENTER, "!", FontId::proportional(11.0), Color32::WHITE);
        }
        for n in p.notes.iter().filter(|n| n.level.eq_ignore_ascii_case(&level)) {
            let at = to([n.at[0], n.at[1]]);
            let col = if n.kind == NoteKind::VerifiedPoint { Color32::from_rgb(40, 180, 120) } else { Color32::from_rgb(70, 140, 240) };
            painter.circle(at, 6.0, col, Stroke::new(1.5, Color32::WHITE));
        }
        self.draw_marks(painter, &|q| Some(to([q[0], q[1]])));
    }

    fn visible_level(&self, level: &str) -> Vec<usize> {
        let Some(p) = &self.pkg else { return Vec::new() };
        p.elements
            .iter()
            .enumerate()
            .filter(|(_, e)| !self.hidden.contains(&e.trade) && e.level.eq_ignore_ascii_case(level) && self.in_section(e))
            .map(|(i, _)| i)
            .take(200_000)
            .collect()
    }

    /// Measure points and the draft note's spot.
    fn draw_marks(&self, painter: &egui::Painter, to: &dyn Fn([f64; 3]) -> Option<Pos2>) {
        let tk = Tokens::get();
        let pts: Vec<Pos2> = self.measure.iter().filter_map(|q| to(*q)).collect();
        if let [a, b] = pts.as_slice() {
            painter.line_segment([*a, *b], Stroke::new(2.0, tk.accent));
            if let [x, y] = self.measure.as_slice() {
                painter.text(
                    pos2((a.x + b.x) * 0.5, (a.y + b.y) * 0.5 - 12.0),
                    Align2::CENTER_BOTTOM,
                    cadaddy_walk3d::feet(Measured::between(*x, *y).dist),
                    FontId::proportional(14.0),
                    tk.accent,
                );
            }
        }
        for q in &pts {
            painter.circle_filled(*q, 4.0, tk.accent);
        }
        if let Some(d) = &self.draft
            && let Some(q) = to(d.at)
        {
            painter.circle(q, 8.0, Color32::TRANSPARENT, Stroke::new(2.0, Color32::from_rgb(70, 140, 240)));
        }
    }

    fn draw_3d(&mut self, painter: &egui::Painter, rect: Rect, cam: &Camera) {
        let tk = Tokens::get();
        let Some(p) = &self.pkg else { return };
        let (w, h) = (f64::from(rect.width()), f64::from(rect.height()));
        let vis = self.visible();
        let walls_seethrough = self.mode == Mode::Model;
        let mut faces: Vec<(f64, [Pos2; 4], Color32)> = Vec::with_capacity(vis.len() * 3);
        for &i in &vis {
            let Some(e) = p.elements.get(i) else { continue };
            let base = rgb(p.trade(&e.trade).color);
            let alpha = if walls_seethrough && e.trade == "arch" && e.kind == "wall" { 0.45 } else { 1.0 };
            let sel = self.selected == Some(i);
            let prisms: Vec<[([[f64; 3]; 4], [f64; 3]); 6]> = if e.path.len() >= 2 {
                let s = e.size_in.unwrap_or([1.0, 1.0]);
                let (w, hh) = ((s[0].max(0.5) / 12.0), (if s[1] > 0.0 { s[1] } else { s[0] }).max(0.5) / 12.0);
                e.path.windows(2).filter_map(|q| Some(prism_faces(*q.first()?, *q.get(1)?, w, hh))).collect()
            } else {
                vec![box_faces(e.min, e.max)]
            };
            for (corners, n) in prisms.into_iter().flatten() {
                let mut q = [Pos2::ZERO; 4];
                let mut depth = 0.0;
                let mut ok = true;
                for (k, c) in corners.iter().enumerate() {
                    match cam.project(*c, w, h) {
                        Some((x, y, z)) => {
                            if let Some(slot) = q.get_mut(k) {
                                *slot = pos2(rect.min.x + x as f32, rect.min.y + y as f32);
                            }
                            depth += z;
                        }
                        None => ok = false,
                    }
                }
                if !ok {
                    continue;
                }
                // Back faces (screen winding) are skipped.
                let area = (q[1].x - q[0].x) * (q[2].y - q[0].y) - (q[2].x - q[0].x) * (q[1].y - q[0].y);
                if area >= 0.0 {
                    continue;
                }
                let light = 0.55 + 0.45 * (n[0] * 0.4 + n[1] * 0.3 + n[2] * 0.85).abs();
                let col = if sel { tk.selection } else { base };
                let shaded = Color32::from_rgba_unmultiplied(
                    (f64::from(col.r()) * light) as u8,
                    (f64::from(col.g()) * light) as u8,
                    (f64::from(col.b()) * light) as u8,
                    (255.0 * alpha) as u8,
                );
                faces.push((depth, q, shaded));
            }
        }
        faces.sort_by(|a, b| b.0.total_cmp(&a.0));
        let mut mesh = egui::Mesh::default();
        for (_, q, col) in &faces {
            let base = mesh.vertices.len() as u32;
            for v in q {
                mesh.colored_vertex(*v, *col);
            }
            mesh.add_triangle(base, base + 1, base + 2);
            mesh.add_triangle(base, base + 2, base + 3);
        }
        painter.add(Shape::mesh(mesh));
        self.drawn_3d = vis.len();
        // Room outlines on the floor, issues and notes as markers.
        let z = self.floor_z();
        let level = self.level_name();
        let to = |q: [f64; 3]| cam.project(q, w, h).map(|(x, y, _)| pos2(rect.min.x + x as f32, rect.min.y + y as f32));
        if let Some(r) = self.room.and_then(|i| p.rooms.get(i)) {
            let pts: Vec<Pos2> = r.outline.iter().filter_map(|q| to([q[0], q[1], z])).collect();
            if pts.len() == r.outline.len() && pts.len() >= 3 {
                painter.add(Shape::closed_line(pts, Stroke::new(2.0, tk.accent)));
            }
        }
        for i in p.issues.iter().filter(|i| i.is_open() && i.level.eq_ignore_ascii_case(&level)) {
            if let Some(at) = to(i.at) {
                painter.circle(
                    at,
                    7.0,
                    if i.kind == IssueKind::Clash { Color32::from_rgb(230, 60, 50) } else { Color32::from_rgb(240, 150, 30) },
                    Stroke::new(1.5, Color32::WHITE),
                );
            }
        }
        for n in p.notes.iter().filter(|n| n.level.eq_ignore_ascii_case(&level)) {
            if let Some(at) = to(n.at) {
                painter.circle(at, 6.0, Color32::from_rgb(70, 140, 240), Stroke::new(1.5, Color32::WHITE));
            }
        }
        self.draw_marks(painter, &to);
    }

    /// The orbit's target moved by a screen drag (pixels): along the view's right and up.
    pub fn pan_orbit(&mut self, by: Vec2) {
        let (y, p) = (self.yaw.to_radians(), self.pitch.clamp(-89.0, 89.0).to_radians());
        let right = [-y.sin(), y.cos(), 0.0];
        // Up on screen: the view's up vector (towards the far side when looking down).
        let up = [-p.sin() * y.cos(), -p.sin() * y.sin(), p.cos()];
        let k = self.dist.max(3.0) * 0.0018;
        let (dx, dy) = (f64::from(by.x) * k, f64::from(by.y) * k);
        for (o, (r, u)) in self.target_off.iter_mut().zip(right.iter().zip(&up)) {
            *o = (*o - r * dx + u * dy).clamp(-50_000.0, 50_000.0);
        }
    }

    /// W A S D (and the arrows) move: walk ahead and turn, slide the orbit, pan the plan; Q and E
    /// turn. Not while a text box has the keyboard.
    fn keys(&mut self, ui: &egui::Ui) {
        if ui.ctx().egui_wants_keyboard_input() {
            return;
        }
        let (ahead, side, turn, run, dt) = ui.input(|i| {
            let k = |a: egui::Key, b: egui::Key| f64::from(u8::from(i.key_down(a) || i.key_down(b)));
            (
                k(egui::Key::W, egui::Key::ArrowUp) - k(egui::Key::S, egui::Key::ArrowDown),
                k(egui::Key::D, egui::Key::ArrowRight) - k(egui::Key::A, egui::Key::ArrowLeft),
                k(egui::Key::Q, egui::Key::Q) - k(egui::Key::E, egui::Key::E),
                i.modifiers.shift,
                f64::from(i.stable_dt).clamp(0.0, 0.1),
            )
        });
        if ahead == 0.0 && side == 0.0 && turn == 0.0 {
            return;
        }
        let speed = if run { 14.0 } else { 6.0 } * dt;
        match self.mode {
            Mode::Walk => {
                // A and D turn when walking (the stick does the same); Q and E too.
                self.heading += (turn - side) * 90.0 * dt;
                let h = self.heading.to_radians();
                self.walk_at = [self.walk_at[0] + h.cos() * ahead * speed, self.walk_at[1] + h.sin() * ahead * speed];
            }
            Mode::Model => {
                self.yaw += turn * 90.0 * dt;
                let y = self.yaw.to_radians();
                let (fwd, right) = ([-y.cos(), -y.sin()], [-y.sin(), y.cos()]);
                let k = speed * (self.dist / 30.0).clamp(1.0, 20.0);
                self.target_off[0] += (fwd[0] * ahead + right[0] * side) * k;
                self.target_off[1] += (fwd[1] * ahead + right[1] * side) * k;
            }
            Mode::Plan => {
                if let Some((c, s)) = &mut self.plan {
                    let k = 400.0 * dt / f64::from(s.max(0.5));
                    c[0] += side * k;
                    c[1] += ahead * k;
                }
            }
        }
        ui.ctx().request_repaint();
    }

    /// A stick at the lower right: held up walks ahead, down back, sideways turns.
    fn walk_stick(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let r = 52.0;
        let centre = pos2(rect.max.x - 24.0 - r, rect.max.y - 24.0 - r);
        let area = Rect::from_center_size(centre, vec2(r * 2.0, r * 2.0));
        let resp = ui.interact(area, ui.id().with("fv_walk_stick"), Sense::drag());
        let tk = Tokens::get();
        let painter = ui.painter_at(rect);
        painter.circle(centre, r, tk.panel.gamma_multiply(0.6), Stroke::new(1.5, tk.text_faint));
        let knob = match resp.interact_pointer_pos().filter(|_| resp.dragged()) {
            Some(at) => {
                let v = at - centre;
                let v = if v.length() > r { v * (r / v.length()) } else { v };
                let dt = f64::from(ui.input(|i| i.stable_dt).clamp(0.0, 0.1));
                let (ahead, turn) = (f64::from(-v.y / r), f64::from(-v.x / r));
                self.heading += turn * 90.0 * dt;
                let h = self.heading.to_radians();
                self.walk_at = [self.walk_at[0] + h.cos() * ahead * 6.0 * dt, self.walk_at[1] + h.sin() * ahead * 6.0 * dt];
                ui.ctx().request_repaint();
                centre + v
            }
            None => centre,
        };
        painter.circle_filled(knob, r * 0.38, tk.accent.gamma_multiply(0.85));
    }

    /// Walk: buttons to step and turn, and a stick to hold (a phone has no keys).
    fn walk_pad(&mut self, ui: &mut egui::Ui, rect: Rect) {
        self.walk_stick(ui, rect);
        let size = vec2(64.0, 44.0);
        let at = |dx: f32, dy: f32| Rect::from_min_size(pos2(rect.min.x + 16.0 + dx, rect.max.y - 16.0 - 3.0 * size.y + dy), size);
        let mut step = 0.0;
        let mut turn = 0.0;
        if ui.put(at(size.x, 0.0), egui::Button::new(t("Ahead"))).clicked() {
            step = 2.0;
        }
        if ui.put(at(0.0, size.y), egui::Button::new(t("Left"))).clicked() {
            turn = 30.0;
        }
        if ui.put(at(2.0 * size.x, size.y), egui::Button::new(t("Right"))).clicked() {
            turn = -30.0;
        }
        if ui.put(at(size.x, 2.0 * size.y), egui::Button::new(t("Back"))).clicked() {
            step = -2.0;
        }
        self.heading += turn;
        let h = self.heading.to_radians();
        self.walk_at = [self.walk_at[0] + h.cos() * step, self.walk_at[1] + h.sin() * step];
    }
}

/// The browser's clock as an ISO-ish text, else the frame time (no clock in tests).
fn now_text(ctx: &egui::Context) -> String {
    let secs = web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    if secs > 0 {
        let days = secs / 86_400;
        let (y, m, d) = civil(days as i64);
        let s = secs % 86_400;
        format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("t{:.0}", ctx.input(|i| i.time))
    }
}

/// Days since 1970-01-01 to a civil date (proleptic Gregorian).
fn civil(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

fn room_box(outline: &[[f64; 2]]) -> Option<([f64; 2], [f64; 2])> {
    if outline.len() < 3 {
        return None;
    }
    let mut lo = [f64::INFINITY; 2];
    let mut hi = [f64::NEG_INFINITY; 2];
    for q in outline {
        lo = [lo[0].min(q[0]), lo[1].min(q[1])];
        hi = [hi[0].max(q[0]), hi[1].max(q[1])];
    }
    Some((lo, hi))
}

/// A run or wall segment from `a` to `b` (centreline), `w` wide and `h` high, as a box turned
/// along it: its six faces as [`box_faces`] gives them.
fn prism_faces(a: [f64; 3], b: [f64; 3], w: f64, h: f64) -> [([[f64; 3]; 4], [f64; 3]); 6] {
    let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-9);
    let x = [d[0] / len, d[1] / len, d[2] / len];
    // Across: horizontal, square to the run (a vertical run gets east).
    let flat = (x[0] * x[0] + x[1] * x[1]).sqrt();
    let y = if flat > 1e-6 { [-x[1] / flat, x[0] / flat, 0.0] } else { [1.0, 0.0, 0.0] };
    let z = [x[1] * y[2] - x[2] * y[1], x[2] * y[0] - x[0] * y[2], x[0] * y[1] - x[1] * y[0]];
    let at = |u: f64, v: f64, s: f64| {
        [a[0] + x[0] * u + y[0] * v + z[0] * s, a[1] + x[1] * u + y[1] * v + z[1] * s, a[2] + x[2] * u + y[2] * v + z[2] * s]
    };
    let mut faces = box_faces([0.0, -w * 0.5, -h * 0.5], [len, w * 0.5, h * 0.5]);
    for (corners, n) in faces.iter_mut() {
        for c in corners.iter_mut() {
            *c = at(c[0], c[1], c[2]);
        }
        *n = [x[0] * n[0] + y[0] * n[1] + z[0] * n[2], x[1] * n[0] + y[1] * n[1] + z[1] * n[2], x[2] * n[0] + y[2] * n[1] + z[2] * n[2]];
    }
    faces
}

/// A box's six faces (corners counter-clockwise seen from outside) and their normals.
fn box_faces(a: [f64; 3], b: [f64; 3]) -> [([[f64; 3]; 4], [f64; 3]); 6] {
    let c = |x: bool, y: bool, z: bool| [if x { b[0] } else { a[0] }, if y { b[1] } else { a[1] }, if z { b[2] } else { a[2] }];
    [
        ([c(false, false, true), c(true, false, true), c(true, true, true), c(false, true, true)], [0.0, 0.0, 1.0]),
        ([c(false, false, false), c(false, true, false), c(true, true, false), c(true, false, false)], [0.0, 0.0, -1.0]),
        ([c(false, false, false), c(true, false, false), c(true, false, true), c(false, false, true)], [0.0, -1.0, 0.0]),
        ([c(false, true, false), c(false, true, true), c(true, true, true), c(true, true, false)], [0.0, 1.0, 0.0]),
        ([c(false, false, false), c(false, false, true), c(false, true, true), c(false, true, false)], [-1.0, 0.0, 0.0]),
        ([c(true, false, false), c(true, true, false), c(true, true, true), c(true, false, true)], [1.0, 0.0, 0.0]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_from_days() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(19_723), (2024, 1, 1));
        assert_eq!(civil(20_513), (2026, 3, 1));
    }

    #[test]
    fn box_faces_point_outwards() {
        let turned = prism_faces([1.0, 1.0, 5.0], [4.0, 5.0, 5.0], 1.0, 2.0);
        for (corners, n) in box_faces([0.0; 3], [1.0, 2.0, 3.0]).into_iter().chain(turned) {
            let [p0, p1, p2, _] = corners;
            let u = [p1[0] - p0[0], p1[1] - p0[1], p1[2] - p0[2]];
            let v = [p2[0] - p0[0], p2[1] - p0[1], p2[2] - p0[2]];
            let cr = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
            assert!(cr[0] * n[0] + cr[1] * n[1] + cr[2] * n[2] > 0.0, "{corners:?} {n:?}");
        }
    }
}
