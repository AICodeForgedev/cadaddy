//! 4D: the model on a schedule. A construction schedule (tasks with a start and a finish date)
//! is linked to the model's elements, by rules (an element filter on each task: level, system,
//! class, name) and by hand (elements added to or taken off a task), and the model is shown as it
//! stands on a date: each element built, in progress or not yet started.
//!
//! An element follows every task it is linked to: not yet until the first of them starts, in
//! progress from then, built once all of them have finished. A task is in progress from its start
//! date through its finish date (both days included) and finished the day after. Elements no task
//! names are left as they are.
//!
//! Dates are days since 1970-01-01 ([`day`] reads them); the schedule keeps them as ISO text
//! (`2026-03-02`). Kept in the drawing as JSON (`Drawing::schedule4d`, the `fourd.*` commands).

use crate::derive::{Owner, Part, Scene};
use cadaddy_doc::{Drawing, Entity, Handle};
use serde::{Deserialize, Serialize};

/// The schedule format written by [`Schedule4d::to_json`].
pub const SCHEDULE_VERSION: u32 = 1;
/// Most tasks kept (hostile files).
pub const MAX_TASKS: usize = 10_000;
/// Most elements a task names by hand.
pub const MAX_PICKS: usize = 100_000;
/// Longest text field kept.
const MAX_TEXT: usize = 200;
/// Most elements linked in all (the sum over tasks).
const MAX_LINKS: usize = 5_000_000;

/// Which elements a task covers: every field given must match (wildcards `*` and `?`, case
/// ignored); a filter with no field matches nothing (the task then has only its hand-picked
/// elements). The class matches with or without IFC's `Ifc` prefix (`Duct`, `IfcDuct*`), the name
/// matches the element's name or its layer.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Filter {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub class: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl Filter {
    pub fn is_empty(&self) -> bool {
        [&self.level, &self.system, &self.class, &self.name].iter().all(|f| f.as_deref().is_none_or(|s| s.trim().is_empty()))
    }

    /// Whether `e` is one of the elements this filter covers.
    pub fn matches(&self, e: &Element) -> bool {
        if self.is_empty() {
            return false;
        }
        fn given(f: &Option<String>) -> Option<&str> {
            f.as_deref().map(str::trim).filter(|s| !s.is_empty())
        }
        if let Some(p) = given(&self.level)
            && !wildcard(p, &e.level)
        {
            return false;
        }
        if let Some(p) = given(&self.system)
            && !e.systems.iter().any(|s| wildcard(p, s))
        {
            return false;
        }
        if let Some(p) = given(&self.class) {
            let bare = e.class.strip_prefix("Ifc").unwrap_or(&e.class);
            if !(wildcard(p, &e.class) || wildcard(p, bare)) {
                return false;
            }
        }
        if let Some(p) = given(&self.name)
            && !(wildcard(p, &e.name) || wildcard(p, &e.layer))
        {
            return false;
        }
        true
    }

    /// The filter as `key=value; …` text (the schedule column it is read from).
    pub fn text(&self) -> String {
        let mut out = Vec::new();
        for (k, v) in [("level", &self.level), ("system", &self.system), ("class", &self.class), ("name", &self.name)] {
            if let Some(v) = v.as_deref().filter(|v| !v.trim().is_empty()) {
                out.push(format!("{k}={v}"));
            }
        }
        out.join("; ")
    }

    /// Read `level=L1; system=Supply*` (`;` or `,` between, `:` also separates a key).
    pub fn parse(text: &str) -> Result<Filter, String> {
        let mut f = Filter::default();
        for part in text.split([';', ',']).map(str::trim).filter(|p| !p.is_empty()).take(16) {
            let Some((k, v)) = part.split_once(['=', ':']) else {
                return Err(format!("`{part}` is not key=value (keys: level, system, class, name)"));
            };
            let v = Some(clip(v.trim()));
            match k.trim().to_ascii_lowercase().as_str() {
                "level" | "storey" | "floor" => f.level = v,
                "system" => f.system = v,
                "class" | "type" | "category" => f.class = v,
                "name" | "element" | "layer" => f.name = v,
                other => return Err(format!("unknown filter key `{other}` (keys: level, system, class, name)")),
            }
        }
        Ok(f)
    }
}

/// One task of the schedule.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Task {
    /// The schedule's own id for the task (its row number when it has none).
    pub id: String,
    pub name: String,
    /// ISO dates, start ≤ finish.
    pub start: String,
    pub finish: String,
    pub filter: Filter,
    /// Elements picked by hand (handles, hex) and elements taken off the filter's.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub add: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub remove: Vec<String>,
}

impl Task {
    pub fn start_day(&self) -> Option<i32> {
        day(&self.start)
    }

    pub fn finish_day(&self) -> Option<i32> {
        day(&self.finish)
    }

    /// Where the task stands on day `d`.
    pub fn phase(&self, d: i32) -> Phase {
        match (self.start_day(), self.finish_day()) {
            (Some(s), _) if d < s => Phase::NotYet,
            (Some(_), Some(f)) if d <= f => Phase::InProgress,
            (Some(_), Some(_)) => Phase::Built,
            _ => Phase::NotYet,
        }
    }
}

/// Where an element (or a task) stands on a date.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    NotYet,
    InProgress,
    Built,
}

impl Phase {
    pub fn name(self) -> &'static str {
        match self {
            Phase::NotYet => "not_yet",
            Phase::InProgress => "in_progress",
            Phase::Built => "built",
        }
    }
}

/// The colour in-progress work is shown in (orange), and the pale grey not-yet work is ghosted in
/// when it is shown at all.
pub const IN_PROGRESS_RGB: [u8; 3] = [240, 140, 20];
pub const NOT_YET_RGB: [u8; 3] = [205, 210, 218];

/// A model element as the schedule's rules see it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Element {
    pub handle: Handle,
    /// The IFC class it is written as (`IfcDuctSegment`), or what Walk made it (`wall`, `door`…).
    pub class: String,
    pub level: String,
    pub systems: Vec<String>,
    pub name: String,
    pub layer: String,
}

/// What a model-space object is as a 4D element: the IFC class IFC export writes it as (the
/// same table as `cadaddy-io`'s IFC writer, checked against it there), its storey, its name (a
/// run's system, a piece of equipment's tag, a room's name, else its layer) and its systems.
pub fn element_of(e: &Entity) -> Element {
    let tags = |app: &str| -> Option<Vec<String>> {
        let x = &e.common.xdata;
        let at = x.iter().position(|t| t.code == 1001 && t.value.trim().eq_ignore_ascii_case(app))?;
        Some(x.iter().skip(at + 1).take_while(|t| t.code != 1001).filter(|t| t.code == 1000).map(|t| t.value.clone()).collect())
    };
    let layer = e.common.layer.clone();
    let mut out =
        Element { handle: e.handle, class: "IfcBuildingElementProxy".into(), level: String::new(), systems: Vec::new(), name: layer.clone(), layer };
    if let Some(t) = tags("CADADDY_WALLGEN") {
        let part = t.first().map(String::as_str).unwrap_or_default();
        let extra = |key: &str| t.iter().skip(3).find_map(|v| v.strip_prefix(key)).map(str::to_string);
        let kind = extra("kind:");
        let mep_part = extra("part:").unwrap_or_default();
        out.level = t.get(1).cloned().unwrap_or_default();
        out.systems = t.iter().skip(3).filter_map(|v| v.strip_prefix("system:")).map(str::to_string).take(32).collect();
        let class = match part {
            "wall" | "parapet" => "IfcWall",
            "floor" => "IfcSlab",
            "ceiling" | "soffit" => "IfcCovering",
            "door" => "IfcDoor",
            "window" => "IfcWindow",
            "stair" => "IfcStair",
            "column" => "IfcColumn",
            "curtainwall" => "IfcCurtainWall",
            "railing" => "IfcRailing",
            "ramp" => "IfcRamp",
            "roof" => "IfcRoof",
            "ground" => "IfcGeographicElement",
            "mep" if kind.as_deref() == Some("equipment") => {
                out.name = extra("tag:").unwrap_or_else(|| mep_part.strip_prefix("equipment:").unwrap_or_default().to_string());
                match mep_part.strip_prefix("equipment:").unwrap_or_default() {
                    "ahu" | "rtu" => "IfcUnitaryEquipment",
                    "vav" => "IfcAirTerminalBox",
                    "exhaust_fan" | "fan" => "IfcFan",
                    "pump" => "IfcPump",
                    "water_heater" => "IfcBoiler",
                    "condensing_unit" => "IfcCondenser",
                    "gas_meter" => "IfcFlowMeter",
                    _ => "IfcBuildingElementProxy",
                }
            }
            "mep" if mep_part.starts_with("terminal:sprinkler") => "IfcFireSuppressionTerminal",
            "mep" if mep_part.starts_with("terminal:") => "IfcAirTerminal",
            "mep" if mep_part.starts_with("fitting:") => match kind.as_deref() {
                Some("pipe") => "IfcPipeFitting",
                Some("conduit" | "tray") => "IfcCableCarrierFitting",
                _ => "IfcDuctFitting",
            },
            "mep" => match kind.as_deref() {
                Some("duct") => "IfcDuctSegment",
                Some("pipe") => "IfcPipeSegment",
                Some("conduit" | "tray") => "IfcCableCarrierSegment",
                Some("light_fixture") => "IfcLightFixture",
                Some("outlet" | "receptacle" | "data_outlet" | "floor_box") => "IfcOutlet",
                Some("switch" | "disconnect") => "IfcSwitchingDevice",
                Some("junction_box") => "IfcJunctionBox",
                Some("panel" | "transformer") => "IfcElectricDistributionBoard",
                _ => "IfcBuildingElementProxy",
            },
            _ => "IfcBuildingElementProxy",
        };
        out.class = class.into();
        if part != "mep" || kind.as_deref() != Some("equipment") {
            out.name = out.systems.first().cloned().unwrap_or_else(|| part.to_string());
        }
    } else if let Some(t) = tags("CADADDY_ROOM") {
        out.class = "IfcSpace".into();
        out.name = t.first().cloned().unwrap_or_default();
    } else if let Some(t) = tags("CADADDY_IFC") {
        // Read from an IFC file: its class as read (`Wall`, `DuctSegment`) and its name.
        if let Some(c) = t.first().filter(|c| !c.is_empty()) {
            out.class = format!("Ifc{}", c.strip_prefix("Ifc").unwrap_or(c));
        }
        if let Some(n) = t.get(2).filter(|n| !n.is_empty()) {
            out.name.clone_from(n);
        }
    }
    out
}

/// The schedule kept in a drawing.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Schedule4d {
    pub version: u32,
    pub tasks: Vec<Task>,
}

/// Which elements each task covers, and which tasks each element follows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Links {
    /// Per task, indices into the elements.
    pub tasks: Vec<Vec<usize>>,
    /// Per element, indices into the tasks.
    pub elements: Vec<Vec<usize>>,
}

impl Links {
    /// Elements no task covers.
    pub fn unlinked(&self) -> usize {
        self.elements.iter().filter(|t| t.is_empty()).count()
    }
}

/// How many elements stand where on a date.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Counts {
    pub built: usize,
    pub in_progress: usize,
    pub not_yet: usize,
    pub unscheduled: usize,
}

impl Schedule4d {
    /// Read a schedule kept in a drawing (`None` = an empty schedule). Capped and checked: tasks
    /// with a bad date are an error.
    pub fn from_json(text: Option<&str>) -> Result<Schedule4d, String> {
        let Some(text) = text.filter(|t| !t.trim().is_empty()) else { return Ok(Schedule4d { version: SCHEDULE_VERSION, tasks: Vec::new() }) };
        let mut s: Schedule4d = serde_json::from_str(text).map_err(|e| format!("the 4D schedule: {e}"))?;
        if s.version > SCHEDULE_VERSION {
            return Err(format!("the 4D schedule is version {}, newer than this CADaddy reads ({SCHEDULE_VERSION})", s.version));
        }
        s.version = SCHEDULE_VERSION;
        s.tasks.truncate(MAX_TASKS);
        for t in &mut s.tasks {
            t.add.truncate(MAX_PICKS);
            t.remove.truncate(MAX_PICKS);
            t.name = clip(&t.name);
            t.id = clip(&t.id);
            check_dates(t)?;
        }
        Ok(s)
    }

    /// The JSON kept in the drawing; `None` without tasks.
    pub fn to_json(&self) -> Option<String> {
        if self.tasks.is_empty() {
            return None;
        }
        serde_json::to_string(&Schedule4d { version: SCHEDULE_VERSION, tasks: self.tasks.clone() }).ok()
    }

    /// The task with id or name `key` (case ignored), ids first.
    pub fn find(&self, key: &str) -> Option<usize> {
        let key = key.trim();
        self.tasks.iter().position(|t| t.id.eq_ignore_ascii_case(key)).or_else(|| self.tasks.iter().position(|t| t.name.eq_ignore_ascii_case(key)))
    }

    /// A schedule from a table's rows (CSV or a spreadsheet sheet): the first row naming a task
    /// column and a start and finish column is the header. Columns, case ignored: `Task`/`Name`/
    /// `Activity` (the name), `ID`, `Start`/`Begin`, `Finish`/`End`, and the filter, either as
    /// `Level`, `System`, `Class`, `Element` columns or one `Filter` column (`level=L1;
    /// system=Supply*`). Returns the schedule and a note for every row left out.
    pub fn from_rows(rows: &[Vec<String>]) -> Result<(Schedule4d, Vec<String>), String> {
        let norm = |s: &str| s.trim().to_ascii_lowercase().replace(['_', '-'], " ");
        let col = |head: &[String], names: &[&str]| head.iter().position(|h| names.contains(&norm(h).as_str()));
        let Some((hi, head)) = rows.iter().enumerate().take(50).find(|(_, r)| {
            col(r, &["task", "task name", "name", "activity", "activity name", "description"]).is_some()
                && col(r, &["start", "begin", "start date", "planned start", "early start"]).is_some()
                && col(r, &["finish", "end", "finish date", "end date", "planned finish", "early finish"]).is_some()
        }) else {
            return Err("no header row with task, start and finish columns".into());
        };
        let c_name = col(head, &["task", "task name", "name", "activity", "activity name", "description"]);
        let c_id = col(head, &["id", "task id", "activity id", "wbs", "code"]);
        let c_start = col(head, &["start", "begin", "start date", "planned start", "early start"]);
        let c_finish = col(head, &["finish", "end", "finish date", "end date", "planned finish", "early finish"]);
        let c_level = col(head, &["level", "storey", "floor"]);
        let c_system = col(head, &["system"]);
        let c_class = col(head, &["class", "type", "category", "element class"]);
        let c_elem = col(head, &["element", "element name", "elements", "layer"]);
        let c_filter = col(head, &["filter", "element filter", "elements filter"]);
        let mut tasks = Vec::new();
        let mut notes = Vec::new();
        for (i, r) in rows.iter().enumerate().skip(hi + 1) {
            let cell = |c: Option<usize>| c.and_then(|c| r.get(c)).map(|s| s.trim()).unwrap_or_default();
            let row_no = i + 1;
            if r.iter().all(|c| c.trim().is_empty()) {
                continue;
            }
            if tasks.len() >= MAX_TASKS {
                notes.push(format!("more than {MAX_TASKS} tasks: the rest are left out"));
                break;
            }
            let name = cell(c_name);
            let (Some(s), Some(f)) = (day(cell(c_start)), day(cell(c_finish))) else {
                notes.push(format!("row {row_no} ({name}): start `{}` or finish `{}` is not a date", cell(c_start), cell(c_finish)));
                continue;
            };
            if f < s {
                notes.push(format!("row {row_no} ({name}): it finishes before it starts"));
                continue;
            }
            let mut filter = match Filter::parse(cell(c_filter)) {
                Ok(f) => f,
                Err(e) => {
                    notes.push(format!("row {row_no} ({name}): {e}"));
                    continue;
                }
            };
            let some = |v: &str| (!v.is_empty()).then(|| clip(v));
            filter.level = some(cell(c_level)).or(filter.level);
            filter.system = some(cell(c_system)).or(filter.system);
            filter.class = some(cell(c_class)).or(filter.class);
            filter.name = some(cell(c_elem)).or(filter.name);
            let id = match cell(c_id) {
                "" => format!("{}", tasks.len() + 1),
                id => clip(id),
            };
            tasks.push(Task { id, name: clip(name), start: iso(s), finish: iso(f), filter, add: Vec::new(), remove: Vec::new() });
        }
        if tasks.is_empty() {
            return Err(match notes.first() {
                Some(n) => format!("no task could be read: {n}"),
                None => "the schedule has no tasks".into(),
            });
        }
        Ok((Schedule4d { version: SCHEDULE_VERSION, tasks }, notes))
    }

    /// First start and last finish.
    pub fn span(&self) -> Option<(i32, i32)> {
        let s = self.tasks.iter().filter_map(Task::start_day).min()?;
        let f = self.tasks.iter().filter_map(Task::finish_day).max()?;
        Some((s, f))
    }

    /// Which elements each task covers: its filter's, plus the ones picked by hand, less the
    /// ones taken off.
    pub fn links(&self, els: &[Element]) -> Links {
        let mut out = Links { tasks: vec![Vec::new(); self.tasks.len()], elements: vec![Vec::new(); els.len()] };
        let by_handle: std::collections::HashMap<Handle, usize> = els.iter().enumerate().map(|(i, e)| (e.handle, i)).collect();
        let handle = |h: &str| u64::from_str_radix(h.trim(), 16).ok().map(Handle);
        let mut total = 0usize;
        for (ti, t) in self.tasks.iter().enumerate() {
            let removed: std::collections::HashSet<Handle> = t.remove.iter().filter_map(|h| handle(h)).collect();
            let mut mine: Vec<usize> = Vec::new();
            if !t.filter.is_empty() {
                mine.extend(els.iter().enumerate().filter(|(_, e)| !removed.contains(&e.handle) && t.filter.matches(e)).map(|(i, _)| i));
            }
            for h in t.add.iter().filter_map(|h| handle(h)) {
                if let Some(&i) = by_handle.get(&h)
                    && !removed.contains(&h)
                {
                    mine.push(i);
                }
            }
            mine.sort_unstable();
            mine.dedup();
            total = total.saturating_add(mine.len());
            if total > MAX_LINKS {
                break;
            }
            for &i in &mine {
                if let Some(e) = out.elements.get_mut(i) {
                    e.push(ti);
                }
            }
            if let Some(slot) = out.tasks.get_mut(ti) {
                *slot = mine;
            }
        }
        out
    }

    /// Where each element stands on day `d` (`None`: no task covers it).
    pub fn phases(&self, links: &Links, d: i32) -> Vec<Option<Phase>> {
        links.elements.iter().map(|ts| element_phase(ts.iter().filter_map(|&t| self.tasks.get(t)).map(|t| t.phase(d)))).collect()
    }

    /// How many elements stand where on day `d`.
    pub fn counts(&self, links: &Links, d: i32) -> Counts {
        let mut c = Counts::default();
        for p in self.phases(links, d) {
            match p {
                Some(Phase::Built) => c.built += 1,
                Some(Phase::InProgress) => c.in_progress += 1,
                Some(Phase::NotYet) => c.not_yet += 1,
                None => c.unscheduled += 1,
            }
        }
        c
    }
}

/// An element's phase from those of the tasks it follows.
fn element_phase(tasks: impl Iterator<Item = Phase>) -> Option<Phase> {
    let (mut any, mut all_built, mut started) = (false, true, false);
    for p in tasks {
        any = true;
        all_built &= p == Phase::Built;
        started |= p != Phase::NotYet;
    }
    any.then_some(if all_built {
        Phase::Built
    } else if started {
        Phase::InProgress
    } else {
        Phase::NotYet
    })
}

fn check_dates(t: &mut Task) -> Result<(), String> {
    let (Some(s), Some(f)) = (day(&t.start), day(&t.finish)) else {
        return Err(format!("task {}: start `{}` or finish `{}` is not a date", t.id, t.start, t.finish));
    };
    if f < s {
        return Err(format!("task {}: it finishes before it starts", t.id));
    }
    (t.start, t.finish) = (iso(s), iso(f));
    Ok(())
}

fn clip(s: &str) -> String {
    s.trim().chars().take(MAX_TEXT).collect()
}

/// The elements of Walk's scene: one per entity a triangle came from, on the open drawing (source
/// 0) only, so hand-picked handles mean the same thing in Walk as in the drawing. The class is
/// the part Walk made of it (`wall`, `door`, `device`…), the level the one its faces sit on, the
/// name a device's block or else its layer. Returns the owners in the same order.
pub fn walk_elements(scene: &Scene, d: &Drawing) -> (Vec<Owner>, Vec<Element>) {
    let mut seen: std::collections::HashMap<Handle, usize> = std::collections::HashMap::new();
    let (mut owners, mut els) = (Vec::new(), Vec::new());
    for (i, o) in scene.owners.iter().enumerate() {
        if o.src != 0 || o.handle == Handle(0) || seen.contains_key(&o.handle) {
            continue;
        }
        let part = scene.parts.get(i).copied().unwrap_or_default();
        let z = scene.tris.get(i * 3).map_or(0.0, |v| f64::from(v.p[2]));
        let level =
            scene.levels.iter().rev().find(|l| z >= l.elevation_ft - 0.01).or(scene.levels.first()).map(|l| l.name.clone()).unwrap_or_default();
        let layer = d.entity(o.handle).map(|e| e.common.layer.clone()).unwrap_or_default();
        let device = scene.devices.iter().find(|dv| dv.handle == o.handle && dv.src == 0);
        let name = device.map_or_else(|| layer.clone(), |dv| dv.block.clone());
        let class = match part {
            Part::Wall => "wall",
            Part::Floor => "floor",
            Part::Ceiling => "ceiling",
            Part::Glass => "window",
            Part::Door => "door",
            Part::Device => "device",
            Part::Stair => "stair",
        };
        seen.insert(o.handle, els.len());
        owners.push(*o);
        els.push(Element { handle: o.handle, class: class.into(), level, systems: Vec::new(), name, layer });
    }
    (owners, els)
}

/// Show Walk's scene as it stands on a date: triangles of not-yet elements are dropped (or
/// ghosted pale grey with `ghost`), in-progress ones turn orange, built and unscheduled ones keep
/// their colours. `phase_of` says where an owner's element stands.
pub fn stage_scene(scene: &Scene, ghost: bool, phase_of: impl Fn(Owner) -> Option<Phase>) -> Scene {
    let mut out = scene.clone();
    let mut keep = vec![true; scene.owners.len()];
    for (i, o) in scene.owners.iter().enumerate() {
        let Some(p) = phase_of(*o) else { continue };
        let rgb = match p {
            Phase::Built => continue,
            Phase::InProgress => IN_PROGRESS_RGB,
            Phase::NotYet if ghost => NOT_YET_RGB,
            Phase::NotYet => {
                if let Some(k) = keep.get_mut(i) {
                    *k = false;
                }
                continue;
            }
        };
        for v in out.tris.iter_mut().skip(i * 3).take(3) {
            v.c = [rgb[0], rgb[1], rgb[2], v.c[3]];
        }
    }
    if keep.iter().any(|k| !k) {
        let tris = std::mem::take(&mut out.tris);
        let (owners, parts) = (std::mem::take(&mut out.owners), std::mem::take(&mut out.parts));
        for (i, tri) in tris.as_chunks::<3>().0.iter().enumerate() {
            if keep.get(i).copied().unwrap_or(true) {
                out.tris.extend_from_slice(tri);
                if let Some(o) = owners.get(i) {
                    out.owners.push(*o);
                }
                if let Some(p) = parts.get(i) {
                    out.parts.push(*p);
                }
            }
        }
    }
    out
}

/// Days since 1970-01-01 of a date: ISO (`2026-03-02`, a time after it ignored), `2026/03/02`,
/// US `3/2/2026` (month first), `2.3.2026` (day first, as written with dots), `2-Mar-2026`,
/// `Mar 2, 2026`, or a spreadsheet's day number (1900 system, 20000–80000).
pub fn day(text: &str) -> Option<i32> {
    let t = text.trim();
    if t.is_empty() || t.len() > 40 {
        return None;
    }
    if let Ok(n) = t.parse::<f64>() {
        // Excel's day 25569 is 1970-01-01.
        return (n.is_finite() && (20_000.0..80_000.0).contains(&n)).then(|| n.floor() as i32 - 25_569);
    }
    let t = t.split(['T', ' ']).next().filter(|d| d.chars().filter(|c| c.is_ascii_digit()).count() >= 6).unwrap_or(t);
    let parts: Vec<&str> = t.split(['-', '/', '.', ' ', ',']).filter(|p| !p.is_empty()).collect();
    let num = |i: usize| parts.get(i).and_then(|p| p.parse::<i32>().ok());
    let month = |i: usize| parts.get(i).and_then(|p| month_of(p));
    let (y, m, d) = match parts.len() {
        3 if parts.first().is_some_and(|p| p.len() == 4) => (num(0)?, num(1)?, num(2)?),
        3 if t.contains('.') => (num(2)?, num(1)?, num(0)?),
        3 if month(0).is_some() => (num(2)?, month(0)?, num(1)?),
        3 if month(1).is_some() => (num(2)?, month(1)?, num(0)?),
        3 => (num(2)?, num(0)?, num(1)?),
        _ => return None,
    };
    let y = if (0..100).contains(&y) { 2000 + y } else { y };
    days_from_civil(y, m, d)
}

fn month_of(s: &str) -> Option<i32> {
    const M: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
    let l = s.to_ascii_lowercase();
    let head = l.get(..3)?;
    M.iter().position(|m| *m == head).and_then(|i| i32::try_from(i + 1).ok())
}

/// Days since 1970-01-01 of a valid proleptic Gregorian date in years 1000–9999.
pub fn days_from_civil(y: i32, m: i32, d: i32) -> Option<i32> {
    if !(1000..=9999).contains(&y) || !(1..=12).contains(&m) || d < 1 || d > month_len(y, m) {
        return None;
    }
    // Howard Hinnant's algorithm.
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

fn month_len(y: i32, m: i32) -> i32 {
    match m {
        2 if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// A day number as an ISO date.
pub fn iso(days: i32) -> String {
    let z = i64::from(days) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// `*` any run, `?` one character; case ignored. Iterative, one back-track point.
pub fn wildcard(pat: &str, text: &str) -> bool {
    let p: Vec<char> = pat.to_lowercase().chars().take(256).collect();
    let t: Vec<char> = text.to_lowercase().chars().take(1024).collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        match (p.get(pi).copied(), t.get(ti).copied()) {
            (Some('*'), _) => {
                star = Some((pi, ti));
                pi += 1;
            }
            (Some(a), Some(b)) if a == '?' || a == b => {
                pi += 1;
                ti += 1;
            }
            _ => match star {
                Some((sp, st)) => {
                    pi = sp + 1;
                    ti = st + 1;
                    star = Some((sp, st + 1));
                }
                None => return false,
            },
        }
    }
    p.iter().skip(pi).all(|&c| c == '*')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn el(h: u64, class: &str, level: &str, system: &str, name: &str) -> Element {
        Element {
            handle: Handle(h),
            class: class.into(),
            level: level.into(),
            systems: if system.is_empty() { Vec::new() } else { vec![system.into()] },
            name: name.into(),
            layer: format!("3D-{name}"),
        }
    }

    fn rows(csv: &str) -> Vec<Vec<String>> {
        csv.lines().map(|l| l.split('|').map(str::to_string).collect()).collect()
    }

    #[test]
    fn dates_read_in_the_usual_ways() {
        let d = days_from_civil(2026, 3, 2).unwrap();
        assert_eq!(days_from_civil(1970, 1, 1), Some(0));
        assert_eq!(iso(d), "2026-03-02");
        for t in [
            "2026-03-02",
            "2026/03/02",
            "3/2/2026",
            "03/02/26",
            "2.3.2026",
            "2-Mar-2026",
            "Mar 2, 2026",
            "March 2 2026",
            "2026-03-02T08:00:00",
            "2026-03-02 17:00",
            "46083",
        ] {
            assert_eq!(day(t), Some(d), "{t}");
        }
        assert_eq!(iso(day("2024-02-29").unwrap()), "2024-02-29");
        for bad in ["", "soon", "2026-02-30", "2026-13-01", "13/45/2026", "1e308", "NaN", "5", "2026-03"] {
            assert_eq!(day(bad), None, "{bad}");
        }
        // Round trip across a span of years.
        for n in (-1000..30_000).step_by(37) {
            assert_eq!(day(&iso(n)), Some(n));
        }
    }

    #[test]
    fn rows_make_tasks_and_bad_rows_are_noted() {
        let r = rows(
            "Project schedule||||\n\
             ID|Task|Start|Finish|Level|System|Class\n\
             A10|Level 1 supply duct|2026-03-02|2026-03-06|Level 1|Supply*|Duct*\n\
             A20|Level 1 diffusers|3/9/2026|3/10/2026|Level 1||AirTerminal\n\
             |||||\n\
             A30|Broken|later|2026-03-10|||\n\
             A40|Backwards|2026-03-10|2026-03-01|||",
        );
        let (s, notes) = Schedule4d::from_rows(&r).unwrap();
        assert_eq!(s.tasks.len(), 2);
        assert_eq!(notes.len(), 2, "{notes:?}");
        let t = &s.tasks[0];
        assert_eq!((t.id.as_str(), t.start.as_str(), t.finish.as_str()), ("A10", "2026-03-02", "2026-03-06"));
        assert_eq!(t.filter.text(), "level=Level 1; system=Supply*; class=Duct*");
        assert_eq!(s.tasks[1].filter.system, None);
        // One filter column instead.
        let r = rows("Activity|Begin|End|Filter\nRoof|2026-05-01|2026-05-20|class=Roof; level=Roof");
        let (s, _) = Schedule4d::from_rows(&r).unwrap();
        assert_eq!(s.tasks[0].filter.class.as_deref(), Some("Roof"));
        assert_eq!(s.tasks[0].id, "1");
        assert!(Schedule4d::from_rows(&rows("Name|Start|Finish\nX|2026-01-01|2026-01-02|\nY|1/1/2026|1/2/2026")).is_ok());
        assert!(Schedule4d::from_rows(&rows("a|b|c\n1|2|3")).is_err(), "no header");
        assert!(Schedule4d::from_rows(&rows("Task|Start|Finish|Filter\nX|2026-01-01|2026-01-02|colour=red")).is_err());
        // JSON round trip.
        let back = Schedule4d::from_json(s.to_json().as_deref()).unwrap();
        assert_eq!(back, s);
        assert!(Schedule4d::from_json(Some(r#"{"version":9,"tasks":[]}"#)).is_err());
        assert!(Schedule4d::from_json(Some(r#"{"version":1,"tasks":[{"id":"1","start":"x","finish":"2026-01-01"}]}"#)).is_err());
        assert!(Schedule4d::from_json(None).unwrap().tasks.is_empty());
    }

    #[test]
    fn links_resolve_by_rule_and_by_hand() {
        let els = vec![
            el(0x10, "IfcDuct", "Level 1", "Supply A", "Supply A"),
            el(0x11, "IfcDuctFitting", "Level 1", "Supply A", "Supply A"),
            el(0x12, "IfcDuct", "Level 2", "Supply A", "Supply A"),
            el(0x13, "IfcAirTerminal", "Level 1", "Supply A", "Supply A"),
            el(0x14, "IfcWall", "Level 1", "", "A-WALL"),
        ];
        let mut s = Schedule4d {
            version: 1,
            tasks: vec![
                Task {
                    id: "A".into(),
                    start: "2026-03-02".into(),
                    finish: "2026-03-06".into(),
                    filter: Filter { level: Some("level 1".into()), class: Some("Duct*".into()), ..Filter::default() },
                    ..Task::default()
                },
                Task {
                    id: "B".into(),
                    start: "2026-03-09".into(),
                    finish: "2026-03-10".into(),
                    filter: Filter { class: Some("IfcAirTerminal".into()), ..Filter::default() },
                    ..Task::default()
                },
                // Hand-picked only.
                Task {
                    id: "C".into(),
                    start: "2026-03-04".into(),
                    finish: "2026-03-12".into(),
                    add: vec!["14".into(), "11".into(), "ff".into()],
                    ..Task::default()
                },
            ],
        };
        let l = s.links(&els);
        assert_eq!(l.tasks, vec![vec![0, 1], vec![3], vec![1, 4]]);
        assert_eq!(l.elements[1], vec![0, 2], "a fitting follows both tasks");
        assert_eq!(l.unlinked(), 1, "level 2's duct");
        // Taking one off.
        s.tasks[0].remove = vec!["10".into()];
        assert_eq!(s.links(&els).tasks[0], vec![1]);
        s.tasks[0].remove.clear();
        // Name matches the layer too; systems.
        let f = Filter { name: Some("3d-a-wall".into()), ..Filter::default() };
        assert!(f.matches(&els[4]) && !f.matches(&els[0]));
        let f = Filter { system: Some("supply*".into()), ..Filter::default() };
        assert_eq!(els.iter().filter(|e| f.matches(e)).count(), 4);
        assert!(!Filter::default().matches(&els[0]), "an empty filter covers nothing");
    }

    #[test]
    fn the_state_at_a_date_is_right() {
        let els = vec![el(1, "IfcDuct", "L1", "S", "S"), el(2, "IfcDuctFitting", "L1", "S", "S"), el(3, "IfcWall", "L1", "", "W")];
        let s = Schedule4d {
            version: 1,
            tasks: vec![
                Task {
                    id: "A".into(),
                    start: "2026-03-02".into(),
                    finish: "2026-03-06".into(),
                    filter: Filter { class: Some("Duct*".into()), ..Filter::default() },
                    ..Task::default()
                },
                Task { id: "B".into(), start: "2026-03-09".into(), finish: "2026-03-10".into(), add: vec!["2".into()], ..Task::default() },
            ],
        };
        let l = s.links(&els);
        let at = |d: &str| s.phases(&l, day(d).unwrap());
        use Phase::*;
        assert_eq!(at("2026-03-01"), vec![Some(NotYet), Some(NotYet), None]);
        assert_eq!(at("2026-03-02"), vec![Some(InProgress), Some(InProgress), None], "the start day is in progress");
        assert_eq!(at("2026-03-06"), vec![Some(InProgress), Some(InProgress), None], "so is the finish day");
        assert_eq!(at("2026-03-07"), vec![Some(Built), Some(InProgress), None], "the fitting waits for task B");
        assert_eq!(at("2026-03-11"), vec![Some(Built), Some(Built), None]);
        assert_eq!(s.counts(&l, day("2026-03-07").unwrap()), Counts { built: 1, in_progress: 1, not_yet: 0, unscheduled: 1 });
        assert_eq!(s.span(), Some((day("2026-03-02").unwrap(), day("2026-03-10").unwrap())));
        assert_eq!(s.find("b"), Some(1));
    }

    #[test]
    fn a_staged_scene_drops_and_tints_triangles() {
        use crate::derive::Vertex;
        let v = |x: f32| Vertex { p: [x, 0.0, 0.0], n: [0.0, 0.0, 1.0], c: [10, 20, 30, 255] };
        let mut sc = Scene::default();
        for h in 1..=3u64 {
            sc.tris.extend([v(h as f32), v(h as f32 + 0.5), v(h as f32 + 0.7)]);
            sc.owners.push(Owner { src: 0, handle: Handle(h) });
            sc.parts.push(Part::Wall);
        }
        let ph = |o: Owner| match o.handle.0 {
            1 => Some(Phase::Built),
            2 => Some(Phase::InProgress),
            _ => Some(Phase::NotYet),
        };
        let st = stage_scene(&sc, false, ph);
        assert_eq!((st.tris.len(), st.owners.len(), st.parts.len()), (6, 2, 2));
        assert_eq!(st.tris[0].c, [10, 20, 30, 255]);
        assert_eq!(st.tris[3].c, [240, 140, 20, 255]);
        let gh = stage_scene(&sc, true, ph);
        assert_eq!(gh.tris.len(), 9);
        assert_eq!(gh.tris[6].c, [205, 210, 218, 255]);
        let (owners, els) = walk_elements(&sc, &Drawing::default());
        assert_eq!(owners.len(), 3);
        assert_eq!(els[0].class, "wall");
    }
}
