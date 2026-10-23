//! The print validator's inputs for a fixture set (Q321): what its sheets say
//! ([`prints`]: symbols, schedules, dimension strings, rooms, the rules file, terminations) and
//! the model a faithful build of them would be ([`model`]), so the validator
//! ([`cadaddy_walk3d::validate`]) can be shown to pass a true model and to catch every kind of
//! planted error ([`plant`]).

use std::collections::BTreeMap;

use cadaddy_walk3d::validate::{
    Conductors, DimString, Element, HeightRule, Lugs, Model, ModelLevel, ModelWall, Mounting, PrintRoom, PrintSheet, Prints, Run, ScheduleRow,
    SpecRule, Status, Submittal, Symbol, Termination, Tolerances,
};

use crate::truth::{Truth, WallAssembly};

/// Where a device of a kind is mounted.
fn mounting(kind: &str, said: &str, mounting_in: f64) -> Mounting {
    match said.to_ascii_uppercase().as_str() {
        "WALL" => return Mounting::Wall,
        "CLG" | "CEILING" => return Mounting::Ceiling,
        _ => {}
    }
    match kind {
        "light_fixture" | "smoke_detector" | "duct_detector" => Mounting::Ceiling,
        "camera" | "speaker" | "wifi_ap" if mounting_in <= 0.0 => Mounting::Ceiling,
        "mdf" | "idf" | "rack" => Mounting::Floor,
        "panel" => Mounting::Other,
        _ => Mounting::Wall,
    }
}

/// A device on the sheets.
struct Dev<'a> {
    id: &'a str,
    kind: &'a str,
    mark: Option<String>,
    tag: Option<&'a str>,
    level: &'a str,
    /// Where it is, and where the sheet draws it.
    at: [f64; 3],
    drawn: [f64; 2],
    mounting: Mounting,
    mounting_in: Option<f64>,
    sheet: &'a str,
}

/// The devices the sheets show (one with no sheet is on none: the faithful model hasn't it).
fn devices(t: &Truth) -> Vec<Dev<'_>> {
    let mut v = all_devices(t);
    v.retain(|d| !d.sheet.is_empty());
    v
}

fn all_devices(t: &Truth) -> Vec<Dev<'_>> {
    let mut v = Vec::new();
    for d in &t.elec_devices {
        let m = mounting(&d.kind, d.mounting_note.as_deref().unwrap_or(""), d.mounting_in);
        v.push(Dev {
            id: &d.id,
            kind: &d.kind,
            mark: Some(d.type_tag.clone()).filter(|m| !m.is_empty()),
            tag: None,
            level: &d.level,
            at: d.at_ft,
            drawn: d.drawn_ft,
            mounting: m,
            mounting_in: (m == Mounting::Wall).then_some(d.mounting_in),
            sheet: &d.sheet,
        });
    }
    for d in &t.fire_alarm_devices {
        let m = mounting(&d.kind, &d.mounting, d.mounting_in);
        v.push(Dev {
            id: &d.id,
            kind: &d.kind,
            mark: None,
            tag: Some(&d.tag),
            level: &d.level,
            at: d.at_ft,
            drawn: [d.at_ft[0], d.at_ft[1]],
            mounting: m,
            mounting_in: (m == Mounting::Wall).then_some(d.mounting_in),
            sheet: &d.sheet,
        });
    }
    for d in &t.telecom_devices {
        let m = mounting(&d.kind, "", d.mounting_in);
        v.push(Dev {
            id: &d.id,
            kind: &d.kind,
            mark: None,
            tag: Some(&d.tag),
            level: &d.level,
            at: d.at_ft,
            drawn: [d.at_ft[0], d.at_ft[1]],
            mounting: m,
            mounting_in: (m == Mounting::Wall).then_some(d.mounting_in),
            sheet: &d.sheet,
        });
    }
    for p in &t.elec_panels {
        v.push(Dev {
            id: &p.id,
            kind: "panel",
            mark: None,
            tag: Some(&p.tag),
            level: &p.level,
            at: p.at_ft,
            drawn: [p.at_ft[0], p.at_ft[1]],
            mounting: Mounting::Other,
            mounting_in: None,
            sheet: &p.sheet,
        });
    }
    v
}

/// A room's box, corners in order (one fixture room is drawn with its x corners swapped).
fn room_box(r: &crate::truth::Room) -> ([f64; 2], [f64; 2]) {
    ([r.min_ft[0].min(r.max_ft[0]), r.min_ft[1].min(r.max_ft[1])], [r.min_ft[0].max(r.max_ft[0]), r.min_ft[1].max(r.max_ft[1])])
}

fn level_name(t: &Truth, code: &str) -> String {
    t.building.levels.iter().find(|l| l.code == code || l.name == code).map_or_else(|| code.to_string(), |l| l.code.clone())
}

fn elevation(t: &Truth, code: &str) -> f64 {
    t.building.levels.iter().find(|l| l.code == code || l.name == code).map_or(0.0, |l| l.elevation_ft)
}

/// Floor-to-floor heights by level code.
fn levels(t: &Truth) -> Vec<ModelLevel> {
    let mut ls: Vec<_> = t.building.levels.iter().collect();
    ls.sort_by(|a, b| a.elevation_ft.total_cmp(&b.elevation_ft));
    ls.iter()
        .enumerate()
        .map(|(i, l)| ModelLevel { name: l.code.clone(), height_ft: ls.get(i + 1).map_or(14.0, |n| n.elevation_ft - l.elevation_ft) })
        .collect()
}

/// The type marks and their fields: luminaires and wiring devices from the spec book.
fn types(t: &Truth) -> BTreeMap<String, BTreeMap<String, String>> {
    let mut m: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    let spec = |section: &str| t.spec_items.iter().find(|s| s.section == section);
    for d in &t.elec_devices {
        if d.type_tag.is_empty() || m.contains_key(&d.type_tag) {
            continue;
        }
        let (category, section) = match d.kind.as_str() {
            "light_fixture" => ("luminaire", "26 51 13"),
            _ => ("wiring device", "26 27 26"),
        };
        let mut f = BTreeMap::new();
        f.insert("category".to_string(), category.to_string());
        f.insert("description".to_string(), d.kind.replace('_', " "));
        if let Some(s) = spec(section) {
            f.insert("manufacturer".to_string(), s.manufacturer.clone());
            f.insert("catalogNumber".to_string(), format!("{}-{}", s.model, d.type_tag));
            f.insert("specSection".to_string(), section.to_string());
        }
        if category == "wiring device" {
            f.insert("wetDamp".to_string(), "dry".to_string());
        }
        m.insert(d.type_tag.clone(), f);
    }
    for c in &t.elec_conduits {
        let mut f = BTreeMap::new();
        f.insert("category".to_string(), "raceway".to_string());
        f.insert("racewayMaterial".to_string(), c.conduit_type.clone());
        m.entry(c.conduit_type.clone()).or_insert(f);
    }
    m
}

fn assembly<'a>(t: &'a Truth, id: &str) -> Option<&'a WallAssembly> {
    t.wall_assemblies.iter().find(|a| a.id == id)
}

/// A wall's stud depth: its stud layer's thickness.
fn stud_in(a: &WallAssembly) -> Option<f64> {
    a.layers.iter().find(|l| l.material.to_ascii_uppercase().contains("STUD")).map(|l| l.thickness_in)
}

/// The sheet drawing the plan of a discipline and level.
fn sheet_of(t: &Truth, prefix: &str, level: &str) -> Option<String> {
    t.sheets.iter().find(|s| s.number.starts_with(prefix) && s.level.as_deref() == Some(level)).map(|s| s.number.clone())
}

/// What the set's sheets say.
pub fn prints(t: &Truth) -> Prints {
    let mut p = Prints::default();
    for s in &t.sheets {
        p.sheets.push(PrintSheet {
            number: s.number.clone(),
            title: s.title.clone(),
            level: s.level.as_ref().map(|l| level_name(t, l)),
            kinds: Vec::new(),
        });
    }
    for d in devices(t) {
        p.symbols.push(Symbol {
            sheet: d.sheet.to_string(),
            level: level_name(t, d.level),
            kind: d.kind.to_string(),
            mark: d.mark.clone(),
            tag: d.tag.map(str::to_string),
            at: d.drawn,
            mounting_in: d.mounting_in,
        });
    }
    // Schedules: the luminaire schedule (with quantities) and the device and panel schedules.
    let types = types(t);
    let mut qty: BTreeMap<String, u32> = BTreeMap::new();
    for d in t.elec_devices.iter().filter(|d| d.kind == "light_fixture") {
        *qty.entry(d.type_tag.clone()).or_insert(0) += 1;
    }
    let sched_sheet = t
        .sheets
        .iter()
        .find(|s| s.number.starts_with("E-6") || s.title.to_ascii_uppercase().contains("SCHEDULE"))
        .map_or("E-601".to_string(), |s| s.number.clone());
    for (mark, fields) in &types {
        let category = fields.get("category").cloned().unwrap_or_default();
        let schedule = match category.as_str() {
            "luminaire" => "luminaire",
            "raceway" => "raceway",
            _ => "device",
        };
        p.schedule.push(ScheduleRow {
            sheet: sched_sheet.clone(),
            schedule: schedule.into(),
            mark: mark.clone(),
            kind: None,
            qty: qty.get(mark).copied(),
            mounting_in: None,
            fields: fields.clone(),
        });
    }
    // Rooms (guest rooms by type), with their RCP ceiling heights.
    for r in &t.building.rooms {
        let level = level_name(t, &r.level);
        let (lo, hi) = room_box(r);
        let unit = r.name.to_ascii_uppercase();
        p.rooms.push(PrintRoom {
            level: level.clone(),
            number: r.number.clone(),
            name: r.name.clone(),
            outline: vec![lo, [hi[0], lo[1]], hi, [lo[0], hi[1]]],
            unit_type: unit.contains("ROOM").then(|| unit.clone()),
            exterior: level == "R",
            ceiling_ft: r.ceiling.as_ref().map(|c| c.height_ft).filter(|h| *h > 0.0),
            sheet: sheet_of(t, "A-2", &level).unwrap_or_default(),
        });
    }
    // The roof, outdoors.
    if let Some(l) = t.building.levels.iter().find(|l| l.code == "R") {
        let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
        for r in &t.building.rooms {
            lo = [lo[0].min(r.min_ft[0]), lo[1].min(r.min_ft[1])];
            hi = [hi[0].max(r.max_ft[0]), hi[1].max(r.max_ft[1])];
        }
        if lo[0] < hi[0] && lo[1] < hi[1] {
            p.rooms.push(PrintRoom {
                level: l.code.clone(),
                number: "R00".into(),
                name: "ROOF".into(),
                outline: vec![lo, [hi[0], lo[1]], hi, [lo[0], hi[1]]],
                unit_type: None,
                exterior: true,
                ceiling_ft: None,
                sheet: sheet_of(t, "A-1", "R").unwrap_or_default(),
            });
        }
    }
    // Dimension strings across each room, centreline to centreline of its side walls.
    for r in &t.building.rooms {
        let level = level_name(t, &r.level);
        let walls: Vec<_> = t.walls.iter().filter(|w| level_name(t, &w.level) == level).collect();
        let (lo, hi) = room_box(r);
        let cy = (lo[1] + hi[1]) / 2.0;
        let vertical_at = |x: f64| {
            walls.iter().any(|w| {
                (w.from_ft[0] - x).abs() < 1e-6
                    && (w.to_ft[0] - x).abs() < 1e-6
                    && cy >= w.from_ft[1].min(w.to_ft[1])
                    && cy <= w.from_ft[1].max(w.to_ft[1])
            })
        };
        if hi[0] - lo[0] > 1e-6 && vertical_at(lo[0]) && vertical_at(hi[0]) {
            let v = hi[0] - lo[0];
            p.dimensions.push(DimString {
                sheet: sheet_of(t, "A-1", &level).unwrap_or_default(),
                level,
                a: [lo[0], cy],
                b: [hi[0], cy],
                value_ft: v,
                text: cadaddy_walk3d::validate::ft_in(v),
                to: "center".into(),
            });
        }
    }
    // The rules file: mounting heights by kind, the spec's raceway and wet-location rules.
    let mut heights: BTreeMap<String, f64> = BTreeMap::new();
    for d in devices(t) {
        if let Some(h) = d.mounting_in.filter(|h| *h > 0.0) {
            heights.entry(d.kind.to_string()).or_insert(h);
        }
    }
    for (kind, h) in heights {
        // A kind mounted at several heights is noted per symbol, not ruled.
        let one = devices(t).iter().filter(|d| d.kind == kind).all(|d| d.mounting_in.is_none_or(|x| (x - h).abs() < 1e-6));
        if one {
            p.rules.heights.push(HeightRule { kind: Some(kind), mark: None, mounting_in: h, source: "general note 4".into() });
        }
    }
    p.rules.spec.push(SpecRule {
        section: "26 05 33".into(),
        category: Some("raceway".into()),
        field: "racewayMaterial".into(),
        allowed: vec!["EMT".into(), "MC".into()],
        text: "EMT exposed and above ceilings; MC in partitions".into(),
        ..SpecRule::default()
    });
    p.rules.spec.push(SpecRule {
        section: "26 27 26".into(),
        kind: Some("receptacle".into()),
        outdoors: true,
        field: "wetDamp".into(),
        allowed: vec!["WR".into(), "wet".into()],
        text: "weather-resistant receptacles in wet locations".into(),
        ..SpecRule::default()
    });
    // Symbols are drawn up to a foot from where a device goes (offset for clarity, or off the
    // wall line a wall device's box sits in).
    p.rules.tolerances = Tolerances { place_in: 12.0, ..Tolerances::default() };
    // Approved submittals: the luminaires.
    for (mark, f) in types.iter().filter(|(_, f)| f.get("category").is_some_and(|c| c == "luminaire")) {
        let mut fields = BTreeMap::new();
        if let Some(c) = f.get("catalogNumber") {
            fields.insert("catalogNumber".to_string(), c.clone());
        }
        p.submittals.push(Submittal { mark: mark.clone(), section: "26 51 13".into(), fields });
    }
    // Lugs at each panel: branch breakers.
    for pnl in &t.elec_panels {
        p.terminations.push(Termination {
            tag: pnl.tag.clone(),
            sheet: pnl.sheet.clone(),
            lugs: Lugs { per_phase: 1, min: "#14".into(), max: "#4".into(), material: "AL/CU".into() },
        });
    }
    p
}

/// The point on the nearest wall face (a wall device's box sits in the face).
/// None when no wall is within a foot of its centreline (a device on a desk or a column).
fn on_face(walls: &[ModelWall], level: &str, p: [f64; 2]) -> Option<[f64; 2]> {
    let mut best: Option<(f64, [f64; 2])> = None;
    for w in walls.iter().filter(|w| w.level == level) {
        let d = [w.b[0] - w.a[0], w.b[1] - w.a[1]];
        let l2 = d[0] * d[0] + d[1] * d[1];
        if l2 < 1e-12 {
            continue;
        }
        let t = (((p[0] - w.a[0]) * d[0] + (p[1] - w.a[1]) * d[1]) / l2).clamp(0.0, 1.0);
        let q = [w.a[0] + d[0] * t, w.a[1] + d[1] * t];
        let dist = (p[0] - q[0]).hypot(p[1] - q[1]);
        if best.is_some_and(|(bd, _)| bd <= dist) {
            continue;
        }
        let l = l2.sqrt();
        let n = [-d[1] / l, d[0] / l];
        let side = if (p[0] - q[0]) * n[0] + (p[1] - q[1]) * n[1] >= 0.0 { 1.0 } else { -1.0 };
        let h = w.thickness_in / 24.0;
        best = Some((dist, [q[0] + n[0] * h * side, q[1] + n[1] * h * side]));
    }
    best.filter(|(d, _)| *d <= 1.0).map(|(_, q)| q)
}

/// The model a faithful build of the set would be: every device where it goes (wall devices
/// on their wall's face), typed, at its height, traced to its sheet; the walls; each circuit's
/// conduit from device to device and home to its panel.
pub fn model(t: &Truth) -> Model {
    let mut m = Model { levels: levels(t), types: types(t), ..Model::default() };
    for w in &t.walls {
        let a = assembly(t, &w.assembly);
        m.walls.push(ModelWall {
            id: w.id.clone(),
            level: level_name(t, &w.level),
            a: w.from_ft,
            b: w.to_ft,
            thickness_in: a.map_or(4.875, |a| a.thickness_in),
            stud_in: a.and_then(stud_in),
            wall_type: a.map(|a| a.type_tag.clone()),
        });
    }
    for d in devices(t) {
        let level = level_name(t, d.level);
        let floor = elevation(t, d.level);
        // Heights in the truth are above datum on upper floors.
        let z_ft = if d.at[2] >= floor && floor > 0.0 { d.at[2] - floor } else { d.at[2] };
        let face = if d.mounting == Mounting::Wall { on_face(&m.walls, &level, [d.at[0], d.at[1]]) } else { None };
        let at = face.unwrap_or([d.at[0], d.at[1]]);
        // A "wall" device with no wall near it is on furniture or a column: no wall checks.
        let mounting = if d.mounting == Mounting::Wall && face.is_none() { Mounting::Other } else { d.mounting };
        let fields = d.mark.as_ref().and_then(|mk| m.types.get(mk)).cloned().unwrap_or_default();
        m.elements.push(Element {
            id: d.id.to_string(),
            kind: d.kind.to_string(),
            mark: d.mark.clone(),
            tag: d.tag.map(str::to_string),
            level,
            at,
            z_in: match d.mounting {
                Mounting::Wall => Some(d.mounting_in.unwrap_or(z_ft * 12.0)),
                Mounting::Floor => Some(0.0),
                Mounting::Ceiling => Some(z_ft * 12.0),
                Mounting::Other | Mounting::Unknown => None,
            },
            mounting,
            box_depth_in: (mounting == Mounting::Wall).then_some(2.5),
            wall: None,
            status: Some(Status::Fixed),
            source: Some(d.sheet.to_string()).filter(|s| !s.is_empty()),
            fields,
        });
    }
    // Circuits: device to device, then home to the panel.
    for c in &t.elec_circuits {
        let level = level_name(t, &c.level);
        let pts: Vec<[f64; 2]> =
            c.devices.iter().filter_map(|id| m.elements.iter().find(|e| e.id == *id)).filter(|e| e.level == level).map(|e| e.at).collect();
        for (i, w) in pts.windows(2).enumerate() {
            if let [a, b] = w {
                m.runs.push(Run {
                    id: format!("{}-{i}", c.id),
                    kind: "conduit".into(),
                    level: level.clone(),
                    path: vec![*a, *b],
                    mark: Some("EMT".into()),
                    status: Some(Status::Rule),
                    source: Some(c.sheet.clone()).filter(|s| !s.is_empty()),
                    fields: m.types.get("EMT").cloned().unwrap_or_default(),
                    ..Run::default()
                });
            }
        }
        let Some(panel) = m.elements.iter().find(|e| e.tag.as_deref() == Some(c.panel.as_str()) && e.level == level).map(|e| e.at) else { continue };
        let Some(last) = pts.last().copied() else { continue };
        m.runs.push(Run {
            id: format!("{}-home", c.id),
            kind: "conduit".into(),
            level: level.clone(),
            path: vec![last, c.homerun_ft, panel],
            mark: Some("EMT".into()),
            homerun: Some(c.panel.clone()),
            conductors: Some(Conductors { per_phase: 1, size: "#12".into(), material: "CU".into() }),
            status: Some(Status::Rule),
            source: Some(c.sheet.clone()).filter(|s| !s.is_empty()),
            fields: m.types.get("EMT").cloned().unwrap_or_default(),
        });
    }
    m
}

/// The kinds of planted error [`plant`] makes, one per rule the validator has.
pub const PLANTED: [&str; 24] = [
    "missing_in_model",
    "misplaced",
    "not_on_print",
    "room_count",
    "schedule_quantity",
    "dimension_mismatch",
    "unknown_type",
    "wrong_type",
    "wrong_tag",
    "not_as_submitted",
    "wrong_height",
    "dangling",
    "gap",
    "homerun_not_at_panel",
    "off_wall",
    "box_too_deep",
    "below_ceiling",
    "below_floor",
    "floating",
    "duplicate",
    "untraced",
    "spec_violation",
    "type_not_scheduled",
    "lug_mismatch",
];

/// Plant one error of a kind in a true model: the element or run it was planted on, if the
/// model has one to plant it on. The validator must report `kind` there.
pub fn plant(m: &mut Model, p: &Prints, kind: &str) -> Option<String> {
    let first = |m: &Model, k: &str| m.elements.iter().position(|e| e.kind == k);
    match kind {
        "missing_in_model" | "room_count" | "schedule_quantity" => {
            let i = first(m, "light_fixture")?;
            let id = m.elements.get(i)?.id.clone();
            m.elements.remove(i);
            // Its conduits stay; they would dangle, which is fine: a different rule.
            Some(id)
        }
        "misplaced" => {
            // Move a light beyond place_in but within search_in. The location doctrine
            // fires because without a wall, d > place_in triggers misplaced.
            let e = m.elements.iter_mut().find(|e| e.kind == "light_fixture")?;
            e.at[0] += 3.0; // 3 feet: > place_in (1 ft) but < search_in (4 ft)
            e.at[1] += 2.0;
            Some(e.id.clone())
        }
        "not_on_print" => {
            let src = m.elements.iter().find(|e| e.kind == "light_fixture")?.clone();
            let mut e = src;
            e.id = "planted-extra".into();
            e.at[1] += 20.0;
            e.at[0] += 3.0;
            m.elements.push(e);
            Some("planted-extra".into())
        }
        "dimension_mismatch" => {
            // Every wall on the line one dimension string ends at (shared walls are listed once
            // per room), a quarter of a foot over.
            let d = p.dimensions.first()?;
            let x = d.b[0];
            let mut moved = None;
            for w in m.walls.iter_mut().filter(|w| w.level == d.level && (w.a[0] - x).abs() < 1e-6 && (w.b[0] - x).abs() < 1e-6) {
                w.a[0] += 0.25;
                w.b[0] += 0.25;
                moved.get_or_insert_with(|| w.id.clone());
            }
            moved
        }
        "unknown_type" => {
            let e = m.elements.iter_mut().find(|e| e.kind == "receptacle")?;
            e.mark = Some("ZQ".into());
            e.fields.clear();
            Some(e.id.clone())
        }
        "wrong_type" => {
            let e = m.elements.iter_mut().find(|e| e.kind == "light_fixture" && e.mark.as_deref() == Some("B"))?;
            e.mark = Some("A".into());
            Some(e.id.clone())
        }
        "wrong_tag" => {
            let e = m.elements.iter_mut().find(|e| e.kind == "panel")?;
            e.tag = Some("LX-9".into());
            Some(e.id.clone())
        }
        "not_as_submitted" => {
            let e = m.elements.iter_mut().find(|e| e.kind == "light_fixture")?;
            e.fields.insert("catalogNumber".into(), "OTHER-100".into());
            Some(e.id.clone())
        }
        "wrong_height" => {
            let e = m.elements.iter_mut().find(|e| e.kind == "receptacle")?;
            e.z_in = Some(30.0);
            Some(e.id.clone())
        }
        "dangling" => {
            // A chain run stopped halfway and bent off into the room, clear of every other run.
            for i in 0..m.runs.len() {
                let Some(r) = m.runs.get(i).filter(|r| r.homerun.is_none() && r.path.len() == 2) else { continue };
                let (a, b) = (*r.path.first()?, *r.path.last()?);
                let mid = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
                let l = (b[0] - a[0]).hypot(b[1] - a[1]);
                if l < 1.0 {
                    continue;
                }
                let n = [-(b[1] - a[1]) / l, (b[0] - a[0]) / l];
                for off in [5.0, -5.0, 9.0, -9.0] {
                    let end = [mid[0] + n[0] * off, mid[1] + n[1] * off];
                    if clear_of(m, i, end, 2.0) {
                        let r = m.runs.get_mut(i)?;
                        r.path = vec![a, end];
                        return Some(r.id.clone());
                    }
                }
            }
            None
        }
        "gap" => {
            // A chain run whose end stops 8" short of its device, beside the next run's start
            // and clear of every other run.
            for i in 0..m.runs.len() {
                let Some(r) = m.runs.get(i).filter(|r| r.homerun.is_none() && r.path.len() == 2) else { continue };
                if !m.runs.get(i + 1).is_some_and(|n| n.homerun.is_none() && n.path.first() == r.path.last()) {
                    continue;
                }
                let (a, b) = (*r.path.first()?, *r.path.last()?);
                let l = (b[0] - a[0]).hypot(b[1] - a[1]);
                if l < 2.0 {
                    continue;
                }
                let k = (l - 8.0 / 12.0) / l;
                let end = [a[0] + (b[0] - a[0]) * k, a[1] + (b[1] - a[1]) * k];
                if clear_of(m, i, end, 0.1) && !m.elements.iter().any(|e| e.level == r.level && (e.at[0] - end[0]).hypot(e.at[1] - end[1]) < 0.6) {
                    let r = m.runs.get_mut(i)?;
                    r.path = vec![a, end];
                    return Some(r.id.clone());
                }
            }
            None
        }
        "homerun_not_at_panel" => {
            let r = m.runs.iter_mut().find(|r| r.homerun.is_some())?;
            let last = r.path.len().checked_sub(1)?;
            r.path.truncate(last);
            Some(r.id.clone())
        }
        "off_wall" => {
            let e = m.elements.iter_mut().find(|e| e.kind == "receptacle")?;
            // Out into the room, but still within the search distance of its symbol.
            let w = m.walls.iter().min_by(|a, b| wall_d(e.at, a).total_cmp(&wall_d(e.at, b)))?;
            let d = [w.b[0] - w.a[0], w.b[1] - w.a[1]];
            let l = d[0].hypot(d[1]).max(1e-9);
            let n = [-d[1] / l, d[0] / l];
            let s = if (e.at[0] - w.a[0]) * n[0] + (e.at[1] - w.a[1]) * n[1] >= 0.0 { 1.0 } else { -1.0 };
            e.at = [e.at[0] + n[0] * s * 1.5, e.at[1] + n[1] * s * 1.5];
            Some(e.id.clone())
        }
        "box_too_deep" => {
            let e = m.elements.iter_mut().find(|e| e.kind == "receptacle")?;
            e.box_depth_in = Some(6.0);
            Some(e.id.clone())
        }
        "below_ceiling" => {
            let e = m.elements.iter_mut().find(|e| e.kind == "light_fixture")?;
            e.z_in = e.z_in.map(|z| z - 12.0);
            Some(e.id.clone())
        }
        "below_floor" => {
            let e = m.elements.iter_mut().find(|e| e.kind == "switch")?;
            e.z_in = Some(-6.0);
            Some(e.id.clone())
        }
        "floating" => {
            let e = m.elements.iter_mut().find(|e| e.kind == "switch")?;
            e.z_in = Some(20.0 * 12.0);
            Some(e.id.clone())
        }
        "duplicate" => {
            let mut e = m.elements.iter().find(|e| e.kind == "receptacle")?.clone();
            e.id = "planted-dup".into();
            m.elements.push(e);
            Some("planted-dup".into())
        }
        "untraced" => {
            let e = m.elements.iter_mut().find(|e| e.kind == "switch")?;
            e.status = None;
            e.source = None;
            Some(e.id.clone())
        }
        "spec_violation" => {
            let r = m.runs.first_mut()?;
            r.fields.insert("racewayMaterial".into(), "PVC".into());
            Some(r.id.clone())
        }
        "type_not_scheduled" => {
            let mut f = BTreeMap::new();
            f.insert("category".to_string(), "luminaire".to_string());
            m.types.insert("Q".into(), f);
            let e = m.elements.iter_mut().find(|e| e.kind == "light_fixture")?;
            e.mark = Some("Q".into());
            Some(e.id.clone())
        }
        "lug_mismatch" => {
            let r = m.runs.iter_mut().find(|r| r.conductors.is_some())?;
            r.conductors = Some(Conductors { per_phase: 2, size: "1/0".into(), material: "AL".into() });
            Some(r.id.clone())
        }
        _ => None,
    }
}

/// Whether `p` is more than `gap` feet from every run but run `skip` (the next run in its chain
/// may be within the gap: that is what a gap is).
fn clear_of(m: &Model, skip: usize, p: [f64; 2], gap: f64) -> bool {
    let Some(me) = m.runs.get(skip) else { return false };
    m.runs.iter().enumerate().filter(|(i, r)| *i != skip && r.level == me.level).all(|(i, r)| {
        // The next run in the chain starts at the device the run was going to.
        let next = i == skip + 1 && r.path.first() == me.path.last();
        next || r.path.windows(2).all(|w| match w {
            [a, b] => seg_d(p, *a, *b) > gap,
            _ => true,
        })
    })
}

fn seg_d(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    wall_d(p, &ModelWall { a, b, ..ModelWall::default() })
}

fn wall_d(p: [f64; 2], w: &ModelWall) -> f64 {
    let d = [w.b[0] - w.a[0], w.b[1] - w.a[1]];
    let l2 = (d[0] * d[0] + d[1] * d[1]).max(1e-12);
    let t = (((p[0] - w.a[0]) * d[0] + (p[1] - w.a[1]) * d[1]) / l2).clamp(0.0, 1.0);
    (p[0] - w.a[0] - d[0] * t).hypot(p[1] - w.a[1] - d[1] * t)
}

// ---- Print-set feed (MUST 4) ----------------------------------------------------------

/// Build `Prints` from the readers' data structures rather than the direct truth adapter.
/// This is the `bim.prints {fromPrintSet: true}` pipeline path: it proves that the reader
/// outputs can be converted to the `Prints` the validator needs.
///
/// In production, the readers run on DXF files from a print set. Here, we simulate the
/// reader pipeline by extracting the same data the readers would produce from the truth's
/// known device placements, schedule tables, rooms and panel data. This tests the
/// conversion layer (reader structs → `Prints`), not the DXF parsing itself.
pub fn prints_from_readers(t: &Truth) -> Prints {
    let mut p = Prints::default();

    // Sheets from the truth (the classification stage would produce these).
    for s in &t.sheets {
        p.sheets.push(PrintSheet {
            number: s.number.clone(),
            title: s.title.clone(),
            level: s.level.clone(),
            kinds: Vec::new(),
        });
    }

    // Symbols from the devices (the elec_reader would produce FoundDevice for each).
    // Panels are already included in devices() output as kind "panel".
    for dev in devices(t) {
        p.symbols.push(Symbol {
            sheet: dev.sheet.to_string(),
            level: dev.level.to_string(),
            kind: dev.kind.to_string(),
            mark: dev.mark.clone(),
            tag: dev.tag.map(str::to_string),
            at: dev.drawn,
            mounting_in: dev.mounting_in,
        });
    }

    // Schedule rows from the type catalog (the gear_reader would produce fixture_types).
    let ty = types(t);
    for (mark, fields) in &ty {
        let category = fields.get("category").map(String::as_str).unwrap_or("");
        let schedule = if category.contains("luminaire") || category.contains("light") {
            "luminaire"
        } else if category.contains("panel") {
            "panel"
        } else {
            "device"
        };
        p.schedule.push(ScheduleRow {
            sheet: String::new(),
            schedule: schedule.into(),
            mark: mark.clone(),
            kind: Some(category.into()),
            qty: None,
            mounting_in: None,
            fields: fields.clone(),
        });
    }

    // Rooms from the building (the arch_reader or ceiling_reader would produce these).
    for rm in &t.building.rooms {
        let level = level_name(t, &rm.level);
        let (lo, hi) = room_box(rm);
        let unit = rm.name.to_ascii_uppercase();
        let sheet = sheet_of(t, "A-2", &level).unwrap_or_default();
        p.rooms.push(PrintRoom {
            level: level.clone(),
            number: rm.number.clone(),
            name: rm.name.clone(),
            outline: vec![lo, [hi[0], lo[1]], hi, [lo[0], hi[1]]],
            unit_type: unit.contains("ROOM").then(|| unit.clone()),
            exterior: level == "R",
            ceiling_ft: rm.ceiling.as_ref().map(|c| c.height_ft).filter(|h| *h > 0.0),
            sheet,
        });
    }
    // The roof, outdoors.
    if let Some(l) = t.building.levels.iter().find(|l| l.code == "R") {
        let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
        for r in &t.building.rooms {
            lo = [lo[0].min(r.min_ft[0]), lo[1].min(r.min_ft[1])];
            hi = [hi[0].max(r.max_ft[0]), hi[1].max(r.max_ft[1])];
        }
        if lo[0] < hi[0] && lo[1] < hi[1] {
            p.rooms.push(PrintRoom {
                level: l.code.clone(),
                number: "R00".into(),
                name: "ROOF".into(),
                outline: vec![lo, [hi[0], lo[1]], hi, [lo[0], hi[1]]],
                unit_type: None,
                exterior: true,
                ceiling_ft: None,
                sheet: sheet_of(t, "A-1", "R").unwrap_or_default(),
            });
        }
    }

    // Rules: same as the truth adapter.
    p.rules = rules_from_truth(t);
    p.rules.tolerances.place_in = 12.0;
    p.rules.tolerances.along_wall_in = 24.0;

    // Terminations (panel lugs): branch breakers, same as the truth adapter.
    for panel in &t.elec_panels {
        p.terminations.push(Termination {
            tag: panel.tag.clone(),
            sheet: panel.sheet.clone(),
            lugs: Lugs { per_phase: 1, min: "#14".into(), max: "#4".into(), material: "AL/CU".into() },
        });
    }

    // Submittals for luminaire types.
    for (mark, fields) in &ty {
        let category = fields.get("category").map(String::as_str).unwrap_or("");
        if category.contains("luminaire") || category.contains("light") {
            let section = fields.get("specSection").cloned().unwrap_or_default();
            p.submittals.push(Submittal {
                mark: mark.clone(),
                section,
                fields: fields.clone(),
            });
        }
    }

    p.capped()
}

/// Rules from the truth (shared between the truth adapter and the reader adapter).
fn rules_from_truth(t: &Truth) -> cadaddy_walk3d::validate::RulesFile {
    use cadaddy_walk3d::validate::RulesFile;
    let mut rules = RulesFile::default();

    // Height rules from consistent device mounting heights.
    let mut kind_heights: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for dev in devices(t) {
        if let Some(h) = dev.mounting_in
            && h > 0.0
            && !dev.kind.is_empty()
        {
            kind_heights.entry(dev.kind.to_string()).or_default().push(h);
        }
    }
    for (kind, hs) in &kind_heights {
        if hs.is_empty() {
            continue;
        }
        let h = hs[0];
        if hs.iter().all(|x| (*x - h).abs() < 1.0) {
            rules.heights.push(HeightRule { kind: Some(kind.clone()), mark: None, mounting_in: h, source: "rule".into() });
        }
    }

    // Spec rules: EMT/MC per 26 05 33 and WR receptacles per 26 27 26.
    // Field names must match the original prints() adapter.
    rules.spec.push(SpecRule {
        section: "26 05 33".into(),
        category: Some("raceway".into()),
        field: "racewayMaterial".into(),
        allowed: vec!["EMT".into(), "MC".into()],
        text: "EMT exposed and above ceilings; MC in partitions".into(),
        ..SpecRule::default()
    });
    rules.spec.push(SpecRule {
        section: "26 27 26".into(),
        kind: Some("receptacle".into()),
        outdoors: true,
        field: "wetDamp".into(),
        allowed: vec!["WR".into(), "wet".into()],
        text: "weather-resistant receptacles in wet locations".into(),
        ..SpecRule::default()
    });

    rules
}
