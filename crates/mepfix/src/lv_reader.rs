//! Low-voltage and fire alarm reader: recognises FA and telecom device symbols on FA/T sheets,
//! equipment connections from schedules, routes LV cables, generates sleeves at crossings,
//! produces per-floor sleeve layout drawings, a sleeve schedule, and LV BOM lines.
//!
//! Symbols on FA sheets (`FA-DEVICE` layer): circles/rectangles with text inside or adjacent
//! (SD, HD, PS, HS, DD, FACP, NAC, ANN, MM, RM).  Symbols on T sheets (`TC-DEVICE` layer):
//! triangles (D, TV, T), circles (WAP, CAM, SPK, NC, ADA), rectangles (CR, REX, IDF, MDF).
//!
//! Position comes from the sheet's coordinate transform (building feet from model inches);
//! heights come from adjacent text (`nn" AFF`) or kind defaults.

use crate::layout;
use crate::truth::{CablePathway, FireAlarmDevice, Room, Sleeve, TelecomDevice, Truth};
use cadaddy_doc::EntityKind;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

// ---- Limits (hostile drawings) -------------------------------------------------------------------

const MAX_ITEMS: usize = 200_000;
const MAX_FA_DEVICES: usize = 50_000;
const MAX_TELECOM_DEVICES: usize = 50_000;
const MAX_SLEEVES: usize = 20_000;

// ---- Default mounting heights (inches AFF) -------------------------------------------------------

fn fa_default_mounting_in(kind: &str) -> f64 {
    match kind {
        "pull_station" => 48.0,
        "horn_strobe" | "speaker_strobe" | "strobe" | "speaker" => 80.0,
        "annunciator" => 60.0,
        "smoke_detector" | "heat_detector" | "co_detector" | "duct_detector" => 0.0,
        "facp" | "nac_panel" => 60.0,
        "monitor_module" | "relay_module" => 60.0,
        "elevator_recall" | "door_holder" | "mag_door_holder" => 84.0,
        _ => 0.0,
    }
}

fn tc_default_mounting_in(kind: &str) -> f64 {
    match kind {
        "data_outlet" | "tv_outlet" | "telephone" => 18.0,
        "card_reader" | "rex" => 48.0,
        "nurse_call_station" | "ada_pull_cord" => 48.0,
        "nurse_call_display" => 60.0,
        "wifi_ap" | "camera" | "speaker" => 0.0,
        "idf" | "mdf" | "patch_panel" | "backboard" | "rack" => 0.0,
        _ => 18.0,
    }
}

// ---- FA device classification from symbol text ---------------------------------------------------

fn classify_fa(text: &str) -> Option<&'static str> {
    match text.trim().to_uppercase().as_str() {
        "SD" => Some("smoke_detector"),
        "HD" => Some("heat_detector"),
        "PS" => Some("pull_station"),
        "HS" => Some("horn_strobe"),
        "SS" => Some("speaker_strobe"),
        "ST" => Some("strobe"),
        "SPK" => Some("speaker"),
        "DD" => Some("duct_detector"),
        "FACP" => Some("facp"),
        "NAC" => Some("nac_panel"),
        "ANN" => Some("annunciator"),
        "MM" => Some("monitor_module"),
        "RM" => Some("relay_module"),
        "CO" => Some("co_detector"),
        _ => None,
    }
}

fn classify_tc(text: &str) -> Option<&'static str> {
    match text.trim().to_uppercase().as_str() {
        "D" => Some("data_outlet"),
        "TV" => Some("tv_outlet"),
        "T" | "TEL" => Some("telephone"),
        "WAP" => Some("wifi_ap"),
        "CAM" => Some("camera"),
        "CR" => Some("card_reader"),
        "REX" => Some("rex"),
        "NC" => Some("nurse_call_station"),
        "ADA" => Some("ada_pull_cord"),
        "ALM" => Some("pool_alarm"),
        "IDF" => Some("idf"),
        "MDF" => Some("mdf"),
        _ => None,
    }
}

// ---- Found devices (reader output) ---------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct FoundFaDevice {
    pub level: String,
    pub kind: String,
    pub tag: String,
    pub room: String,
    pub at_ft: [f64; 3],
    pub mounting: String,
    pub mounting_in: f64,
    pub height_source: String,
    pub candela: Option<u32>,
    pub slc: String,
    pub nac: String,
    pub sheet: String,
    pub source: String,
    pub status: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct FoundTelecomDevice {
    pub level: String,
    pub kind: String,
    pub tag: String,
    pub room: String,
    pub at_ft: [f64; 3],
    pub mounting_in: f64,
    pub lv_system: String,
    pub cable_spec: Option<String>,
    pub sheet: String,
    pub source: String,
    pub status: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct FoundEquipConnection {
    pub level: String,
    pub kind: String,
    pub tag: String,
    pub circuit: String,
    pub panel: String,
    pub connection_type: String,
    pub schedule: BTreeMap<String, String>,
    pub sheet: String,
    pub source: String,
    pub status: String,
}

/// A review item raised by the reader for incomplete data.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ReviewItem {
    pub kind: String,
    pub severity: u8,
    pub device_id: String,
    pub device_kind: String,
    pub level: String,
    pub at_ft: [f64; 3],
    pub issue: String,
    pub suggested: String,
}

// ---- Score result ---------------------------------------------------------------------------------

#[derive(Clone, Debug, Default, Serialize)]
pub struct DeviceScore {
    pub precision: f64,
    pub recall: f64,
    pub count_found: usize,
    pub count_truth: usize,
    pub count_matched: usize,
    pub candela_exact: f64,
    pub height_exact: f64,
}

// ---- Coordinate helpers ---------------------------------------------------------------------------

fn building_from_model(p: &crate::truth::Placement, m: [f64; 2]) -> [f64; 2] {
    let pt = [m[0] - p.offset_in[0], m[1] - p.offset_in[1]];
    let r = layout::rotate(pt, -p.rotation_deg);
    [r[0] / 12.0, r[1] / 12.0]
}

// ---- FA reader ------------------------------------------------------------------------------------

pub fn read_fa_devices(dxf_data: &[(String, Vec<u8>)], truth: &Truth) -> (Vec<FoundFaDevice>, Vec<ReviewItem>) {
    let mut found = Vec::new();
    let mut reviews = Vec::new();
    let rooms = room_map(truth);

    for (name, bytes) in dxf_data {
        let sheet_num = name.trim_end_matches(".dxf").trim_end_matches(".DXF");
        if !sheet_num.starts_with("FA-") {
            continue;
        }
        let sh = match truth.sheets.iter().find(|s| s.number == sheet_num) {
            Some(s) => s,
            None => continue,
        };
        let level = sh.level.clone().unwrap_or_default();
        if level.is_empty() {
            continue;
        }
        let d = match cadaddy_io::read_dxf(bytes) {
            Ok(d) => d,
            Err(_) => continue,
        };
        let placement = &sh.model_from_building;

        let (texts, shapes) = collect_fa_entities(&d);
        let tol = 120.0 / sh.scale;

        for spos in &shapes {
            let kind = match find_classified_text(&texts, *spos, tol, classify_fa) {
                Some(k) => k,
                None => continue,
            };
            let bpt = building_from_model(placement, *spos);
            let mounting_in = fa_default_mounting_in(kind);
            let z_ft = if mounting_in == 0.0 { level_height_ft(truth, &level) } else { mounting_in / 12.0 };
            let room = find_room(&rooms, &level, [bpt[0], bpt[1]]);
            let candela = find_candela_note(&texts, *spos, tol);
            let slc = find_circuit_note(&texts, *spos, tol, "SLC");
            let nac_note = find_circuit_note(&texts, *spos, tol, "NAC");
            let (final_height, height_source) = find_height_note(&texts, *spos, tol).unwrap_or((mounting_in, "rule".into()));
            let final_mounting = if final_height == 0.0 { "CLG" } else { "WALL" };
            let tag = find_tag_note(&texts, *spos, tol, classify_fa, classify_tc).unwrap_or_default();

            // Review items (MUST 3).
            if matches!(kind, "horn_strobe" | "speaker_strobe" | "strobe" | "speaker") && candela.is_none() {
                reviews.push(ReviewItem {
                    kind: "missing_tag".into(),
                    severity: 3,
                    device_id: format!("fa-{}-{}", level, found.len()),
                    device_kind: kind.into(),
                    level: level.clone(),
                    at_ft: [bpt[0], bpt[1], z_ft],
                    issue: "FA notification appliance missing candela rating".into(),
                    suggested: "Add candela (cd) note to device symbol".into(),
                });
            }
            if final_mounting != "CLG" && height_source == "rule" {
                reviews.push(ReviewItem {
                    kind: "missing_tag".into(),
                    severity: 2,
                    device_id: format!("fa-{}-{}", level, found.len()),
                    device_kind: kind.into(),
                    level: level.clone(),
                    at_ft: [bpt[0], bpt[1], z_ft],
                    issue: "FA device missing mounting height note".into(),
                    suggested: "Add height AFF note to device".into(),
                });
            }

            if found.len() < MAX_FA_DEVICES {
                found.push(FoundFaDevice {
                    level: level.clone(),
                    kind: kind.into(),
                    tag,
                    room,
                    at_ft: [bpt[0], bpt[1], z_ft],
                    mounting: final_mounting.into(),
                    mounting_in: final_height,
                    height_source,
                    candela,
                    slc,
                    nac: nac_note,
                    sheet: sheet_num.into(),
                    source: sheet_num.into(),
                    status: "read".into(),
                });
            }
        }
    }
    (found, reviews)
}

fn collect_fa_entities(d: &cadaddy_doc::Drawing) -> (Vec<([f64; 2], String)>, Vec<[f64; 2]>) {
    let mut texts = Vec::new();
    let mut shapes = Vec::new();
    for e in d.model.iter().take(MAX_ITEMS) {
        let layer = &e.common.layer;
        let is_fa = layer.contains("FA-") || layer.contains("FIRE") || layer.contains("ALARM") || layer.contains("TEXT");
        if !is_fa {
            continue;
        }
        match &e.kind {
            EntityKind::Text(t) => {
                let txt = t.value.trim().to_string();
                if !txt.is_empty() {
                    texts.push(([t.insert.x, t.insert.y], txt));
                }
            }
            EntityKind::MText(mt) => {
                let txt = mt.contents.trim().to_string();
                if !txt.is_empty() {
                    texts.push(([mt.insert.x, mt.insert.y], txt));
                }
            }
            EntityKind::Circle(c) => {
                shapes.push([c.center.x, c.center.y]);
            }
            EntityKind::LwPolyline(p) if !p.vertices.is_empty() => {
                let (sx, sy): (f64, f64) = p.vertices.iter().fold((0.0, 0.0), |(x, y), v| (x + v.p.x, y + v.p.y));
                let n = p.vertices.len() as f64;
                shapes.push([sx / n, sy / n]);
            }
            _ => {}
        }
    }
    (texts, shapes)
}

// ---- Telecom reader -------------------------------------------------------------------------------

pub fn read_telecom_devices(dxf_data: &[(String, Vec<u8>)], truth: &Truth) -> Vec<FoundTelecomDevice> {
    let mut found = Vec::new();
    let rooms = room_map(truth);

    for (name, bytes) in dxf_data {
        let sheet_num = name.trim_end_matches(".dxf").trim_end_matches(".DXF");
        if !sheet_num.starts_with("T-") {
            continue;
        }
        let sh = match truth.sheets.iter().find(|s| s.number == sheet_num) {
            Some(s) => s,
            None => continue,
        };
        let level = sh.level.clone().unwrap_or_default();
        if level.is_empty() {
            continue;
        }
        let d = match cadaddy_io::read_dxf(bytes) {
            Ok(d) => d,
            Err(_) => continue,
        };
        let placement = &sh.model_from_building;

        let (texts, shapes) = collect_tc_entities(&d);
        let tol = 120.0 / sh.scale;

        for spos in &shapes {
            let kind = match find_classified_text(&texts, *spos, tol, classify_tc) {
                Some(k) => k,
                None => continue,
            };
            let bpt = building_from_model(placement, *spos);
            let mounting_in = tc_default_mounting_in(kind);
            let z_ft = if mounting_in == 0.0 { level_height_ft(truth, &level) } else { mounting_in / 12.0 };
            let room = find_room(&rooms, &level, [bpt[0], bpt[1]]);
            let lv_system = lv_system_for(kind);
            let cable = find_cable_note(&texts, *spos, tol);
            let tag = find_tag_note(&texts, *spos, tol, classify_fa, classify_tc).unwrap_or_default();

            if found.len() < MAX_TELECOM_DEVICES {
                found.push(FoundTelecomDevice {
                    level: level.clone(),
                    kind: kind.into(),
                    tag,
                    room,
                    at_ft: [bpt[0], bpt[1], z_ft],
                    mounting_in,
                    lv_system: lv_system.into(),
                    cable_spec: cable,
                    sheet: sheet_num.into(),
                    source: sheet_num.into(),
                    status: "read".into(),
                });
            }
        }
    }
    found
}

fn collect_tc_entities(d: &cadaddy_doc::Drawing) -> (Vec<([f64; 2], String)>, Vec<[f64; 2]>) {
    let mut texts = Vec::new();
    let mut shapes = Vec::new();
    for e in d.model.iter().take(MAX_ITEMS) {
        let layer = &e.common.layer;
        let is_tc = layer.contains("TC-") || layer.contains("TELECOM") || layer.contains("DATA") || layer.contains("TEXT");
        if !is_tc {
            continue;
        }
        match &e.kind {
            EntityKind::Text(t) => {
                let txt = t.value.trim().to_string();
                if !txt.is_empty() {
                    texts.push(([t.insert.x, t.insert.y], txt));
                }
            }
            EntityKind::MText(mt) => {
                let txt = mt.contents.trim().to_string();
                if !txt.is_empty() {
                    texts.push(([mt.insert.x, mt.insert.y], txt));
                }
            }
            EntityKind::Circle(c) => {
                shapes.push([c.center.x, c.center.y]);
            }
            EntityKind::LwPolyline(p) if !p.vertices.is_empty() => {
                let (sx, sy): (f64, f64) = p.vertices.iter().fold((0.0, 0.0), |(x, y), v| (x + v.p.x, y + v.p.y));
                let n = p.vertices.len() as f64;
                shapes.push([sx / n, sy / n]);
            }
            _ => {}
        }
    }
    (texts, shapes)
}

// ---- Equipment connection reader ------------------------------------------------------------------

pub fn read_equip_connections(dxf_data: &[(String, Vec<u8>)], truth: &Truth) -> Vec<FoundEquipConnection> {
    let mut found = Vec::new();
    for (name, bytes) in dxf_data {
        let sheet_num = name.trim_end_matches(".dxf").trim_end_matches(".DXF");
        if !sheet_num.contains("E-6") && !sheet_num.contains("E-20") {
            continue;
        }
        let sh = match truth.sheets.iter().find(|s| s.number == sheet_num) {
            Some(s) => s,
            None => continue,
        };
        let d = match cadaddy_io::read_dxf(bytes) {
            Ok(d) => d,
            Err(_) => continue,
        };
        let mut texts: Vec<([f64; 2], String)> = Vec::new();
        for e in d.model.iter().take(MAX_ITEMS) {
            match &e.kind {
                EntityKind::Text(t) => {
                    let txt = t.value.trim().to_string();
                    if !txt.is_empty() {
                        texts.push(([t.insert.x, t.insert.y], txt));
                    }
                }
                EntityKind::MText(mt) => {
                    let txt = mt.contents.trim().to_string();
                    if !txt.is_empty() {
                        texts.push(([mt.insert.x, mt.insert.y], txt));
                    }
                }
                _ => {}
            }
        }
        for (pos, txt) in &texts {
            let upper = txt.to_uppercase();
            let kind = if upper.contains("PTAC") {
                "ptac"
            } else if upper.contains("VTAC") {
                "vtac"
            } else if upper.contains("RTU") {
                "rtu"
            } else if upper.contains("WATER HEATER") || upper.starts_with("WH-") {
                "water_heater"
            } else if upper.contains("ELEVATOR") {
                "elevator"
            } else if upper.contains("FIRE PUMP") {
                "fire_pump"
            } else if upper.contains("POOL PUMP") {
                "pool_pump"
            } else if upper.contains("POOL HEATER") {
                "pool_heater"
            } else if upper.contains("KITCHEN") {
                "kitchen_equip"
            } else if upper.contains("WASHER") {
                "washer"
            } else if upper.contains("DRYER") {
                "dryer"
            } else if upper.contains("SIGNAGE") {
                "signage"
            } else {
                continue;
            };
            let tol = 200.0;
            let circuit = find_nearby_text(&texts, *pos, tol, &["CKT", "CIRCUIT"]);
            let panel = find_nearby_text(&texts, *pos, tol, &["PANEL", "PNL"]);
            let conn = if find_nearby_text(&texts, *pos, tol, &["DISCONNECT", "DS"]).is_some() {
                "disconnect"
            } else if find_nearby_text(&texts, *pos, tol, &["RECEPTACLE"]).is_some() {
                "receptacle"
            } else if find_nearby_text(&texts, *pos, tol, &["JB", "JUNCTION"]).is_some() {
                "junction_box"
            } else {
                "whip"
            };
            if found.len() < MAX_FA_DEVICES {
                found.push(FoundEquipConnection {
                    level: sh.level.clone().unwrap_or_default(),
                    kind: kind.into(),
                    tag: String::new(),
                    circuit: circuit.unwrap_or_default(),
                    panel: panel.unwrap_or_default(),
                    connection_type: conn.into(),
                    schedule: BTreeMap::new(),
                    sheet: sheet_num.into(),
                    source: sheet_num.into(),
                    status: "read".into(),
                });
            }
        }
    }
    found
}

// ---- Score against truth --------------------------------------------------------------------------

pub fn score_fa(found: &[FoundFaDevice], truth_devs: &[FireAlarmDevice], tol_ft: f64) -> DeviceScore {
    if truth_devs.is_empty() {
        return DeviceScore::default();
    }
    let mut matched = 0usize;
    let mut candela_match = 0usize;
    let mut height_match = 0usize;
    let mut used = vec![false; truth_devs.len()];
    for f in found {
        let mut best_idx = None;
        let mut best_dist = f64::MAX;
        for (i, t) in truth_devs.iter().enumerate() {
            if used[i] || t.level != f.level || t.kind != f.kind {
                continue;
            }
            let d = ((f.at_ft[0] - t.at_ft[0]).powi(2) + (f.at_ft[1] - t.at_ft[1]).powi(2)).sqrt();
            if d <= tol_ft && d < best_dist {
                best_dist = d;
                best_idx = Some(i);
            }
        }
        if let Some(i) = best_idx {
            used[i] = true;
            matched += 1;
            if f.candela == truth_devs[i].candela {
                candela_match += 1;
            }
            if (f.mounting_in - truth_devs[i].mounting_in).abs() < 1.0 {
                height_match += 1;
            }
        }
    }
    let n = found.len().max(1) as f64;
    let tn = truth_devs.len().max(1) as f64;
    let mn = matched.max(1) as f64;
    DeviceScore {
        precision: matched as f64 / n,
        recall: matched as f64 / tn,
        count_found: found.len(),
        count_truth: truth_devs.len(),
        count_matched: matched,
        candela_exact: candela_match as f64 / mn,
        height_exact: height_match as f64 / mn,
    }
}

pub fn score_tc(found: &[FoundTelecomDevice], truth_devs: &[TelecomDevice], tol_ft: f64) -> DeviceScore {
    if truth_devs.is_empty() {
        return DeviceScore::default();
    }
    let mut matched = 0usize;
    let mut used = vec![false; truth_devs.len()];
    for f in found {
        let mut best_idx = None;
        let mut best_dist = f64::MAX;
        for (i, t) in truth_devs.iter().enumerate() {
            if used[i] || t.level != f.level || t.kind != f.kind {
                continue;
            }
            let d = ((f.at_ft[0] - t.at_ft[0]).powi(2) + (f.at_ft[1] - t.at_ft[1]).powi(2)).sqrt();
            if d <= tol_ft && d < best_dist {
                best_dist = d;
                best_idx = Some(i);
            }
        }
        if let Some(i) = best_idx {
            used[i] = true;
            matched += 1;
        }
    }
    let n = found.len().max(1) as f64;
    let tn = truth_devs.len().max(1) as f64;
    DeviceScore {
        precision: matched as f64 / n,
        recall: matched as f64 / tn,
        count_found: found.len(),
        count_truth: truth_devs.len(),
        count_matched: matched,
        candela_exact: 0.0,
        height_exact: 0.0,
    }
}

// ---- Sleeve routing (MUST 4) ----------------------------------------------------------------------

fn cable_od_in(cable: &str) -> f64 {
    let u = cable.to_uppercase();
    if u.contains("CAT6A") {
        0.30
    } else if u.contains("CAT6") {
        0.25
    } else if u.contains("CAT5") {
        0.20
    } else if u.contains("RG6") {
        0.27
    } else if u.contains("FPL") {
        0.22
    } else if u.contains("FIBER") {
        0.15
    } else {
        0.25
    }
}

pub fn compute_sleeve_size_in(cables: &[String], spare_pct: f64) -> f64 {
    let mut total_area = 0.0f64;
    for cable in cables {
        let (count, od) = parse_cable_count_and_od(cable);
        total_area += count as f64 * std::f64::consts::PI * (od / 2.0) * (od / 2.0);
    }
    let fill = 0.40;
    let needed_area = total_area / fill * (1.0 + spare_pct / 100.0);
    let needed_dia = 2.0 * (needed_area / std::f64::consts::PI).sqrt();
    next_trade_size(needed_dia)
}

fn parse_cable_count_and_od(cable: &str) -> (u32, f64) {
    let parts: Vec<&str> = cable.split(&['x', 'X', '\u{00d7}'][..]).collect();
    if parts.len() >= 2 { (parts[0].trim().parse::<u32>().unwrap_or(1), cable_od_in(parts[1].trim())) } else { (1, cable_od_in(cable)) }
}

fn next_trade_size(dia_in: f64) -> f64 {
    for &s in &[0.5, 0.75, 1.0, 1.25, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0_f64] {
        if s >= dia_in {
            return s;
        }
    }
    4.0
}

pub fn route_sleeves(truth: &Truth) -> (Vec<Sleeve>, Vec<CablePathway>) {
    let mut sleeves = Vec::new();
    let mut pathways = Vec::new();
    let levels: BTreeSet<&str> = truth.building.levels.iter().map(|l| l.code.as_str()).collect();
    let asm_map: BTreeMap<&str, &crate::truth::WallAssembly> = truth.wall_assemblies.iter().map(|a| (a.type_tag.as_str(), a)).collect();

    for level in &levels {
        let idfs: Vec<&TelecomDevice> =
            truth.telecom_devices.iter().filter(|d| d.level == *level && matches!(d.kind.as_str(), "idf" | "mdf" | "patch_panel")).collect();
        if idfs.is_empty() {
            continue;
        }
        let idf_pos = [idfs[0].at_ft[0], idfs[0].at_ft[1]];

        let rated_walls: Vec<(&crate::truth::WallInstance, &crate::truth::WallAssembly)> = truth
            .walls
            .iter()
            .filter(|w| w.level == *level)
            .filter_map(|w| asm_map.get(w.assembly.as_str()).filter(|a| !a.fire_rating.is_empty()).map(|a| (w, *a)))
            .collect();

        for fa in truth.fire_alarm_devices.iter().filter(|d| d.level == *level) {
            let cables = fa_cables(fa);
            let dev_pos = [fa.at_ft[0], fa.at_ft[1]];
            for (wall, asm) in &rated_walls {
                if let Some(cp) = seg_intersect(dev_pos, idf_pos, wall.from_ft, wall.to_ft) {
                    if sleeves.len() >= MAX_SLEEVES {
                        break;
                    }
                    sleeves.push(Sleeve {
                        id: format!("sl-{}-fa-{}", level, sleeves.len()),
                        systems: vec!["fire_alarm".into()],
                        level: level.to_string(),
                        at_ft: [cp[0], cp[1], 0.0],
                        penetrates: "rated_wall".into(),
                        element_id: Some(wall.id.clone()),
                        material: "EMT".into(),
                        size_in: compute_sleeve_size_in(&cables, 25.0),
                        length_in: asm.thickness_in,
                        fire_rating: asm.fire_rating.clone(),
                        firestop_system: Some("FS-101".into()),
                        cables: cables.clone(),
                        sheet: String::new(),
                    });
                }
            }
        }
        for tc in truth
            .telecom_devices
            .iter()
            .filter(|d| d.level == *level && !matches!(d.kind.as_str(), "idf" | "mdf" | "patch_panel" | "backboard" | "rack"))
        {
            let cables = tc_cables(tc);
            let dev_pos = [tc.at_ft[0], tc.at_ft[1]];
            for (wall, asm) in &rated_walls {
                if let Some(cp) = seg_intersect(dev_pos, idf_pos, wall.from_ft, wall.to_ft) {
                    if sleeves.len() >= MAX_SLEEVES {
                        break;
                    }
                    sleeves.push(Sleeve {
                        id: format!("sl-{}-tc-{}", level, sleeves.len()),
                        systems: vec![tc.lv_system.clone()],
                        level: level.to_string(),
                        at_ft: [cp[0], cp[1], 0.0],
                        penetrates: "rated_wall".into(),
                        element_id: Some(wall.id.clone()),
                        material: "ENT".into(),
                        size_in: compute_sleeve_size_in(&cables, 25.0),
                        length_in: asm.thickness_in,
                        fire_rating: asm.fire_rating.clone(),
                        firestop_system: Some("FS-101".into()),
                        cables: cables.clone(),
                        sheet: String::new(),
                    });
                }
            }
        }

        // J-hook pathways along corridors.
        for room in truth.building.rooms.iter().filter(|r| r.level == *level && r.name.to_uppercase().contains("CORRIDOR")) {
            pathways.push(CablePathway {
                id: format!("cp-{}-{}", level, pathways.len()),
                kind: "j_hook".into(),
                systems: vec!["data".into(), "fire_alarm".into(), "security".into()],
                level: level.to_string(),
                from_ft: [room.min_ft[0], (room.min_ft[1] + room.max_ft[1]) / 2.0],
                to_ft: [room.max_ft[0], (room.min_ft[1] + room.max_ft[1]) / 2.0],
                elevation_ft: 9.0,
                size_in: None,
                spacing_in: Some(48.0),
                tray_size_in: None,
                sheet: String::new(),
            });
        }

        // Floor sleeves for risers.
        for (sys, cables) in &[
            ("data", vec!["12 x Cat6".to_string(), "4 x RG6".to_string()]),
            ("fire_alarm", vec!["2 x FPL".to_string()]),
            ("security", vec!["4 x Cat6".to_string()]),
        ] {
            if sleeves.len() >= MAX_SLEEVES {
                break;
            }
            sleeves.push(Sleeve {
                id: format!("sl-{}-floor-{}", level, sleeves.len()),
                systems: vec![sys.to_string()],
                level: level.to_string(),
                at_ft: [idf_pos[0], idf_pos[1], 0.0],
                penetrates: "floor".into(),
                element_id: None,
                material: "EMT".into(),
                size_in: compute_sleeve_size_in(cables, 25.0),
                length_in: 12.0,
                fire_rating: "2-HR".into(),
                firestop_system: Some("FS-101".into()),
                cables: cables.clone(),
                sheet: String::new(),
            });
        }
    }
    (sleeves, pathways)
}

// ---- LV BOM (MUST 6) -----------------------------------------------------------------------------

#[derive(Clone, Debug, Default, Serialize)]
pub struct LvBomEntry {
    pub system: String,
    pub category: String,
    pub description: String,
    pub quantity: f64,
    pub unit: String,
    pub assumed: bool,
    pub scope: String,
    pub spec_id: Option<String>,
}

pub fn generate_lv_bom(truth: &Truth, has_lv_docs: bool) -> Vec<LvBomEntry> {
    let mut lines = Vec::new();
    if !has_lv_docs {
        lines.push(LvBomEntry {
            system: "general".into(),
            category: "misc".into(),
            description: "LV drawings needed - document request".into(),
            quantity: 1.0,
            unit: "LOT".into(),
            assumed: true,
            scope: "ours".into(),
            spec_id: None,
        });
        for sys in &["data", "fire_alarm", "security", "access_control"] {
            lines.push(LvBomEntry {
                system: sys.to_string(),
                category: "device".into(),
                description: format!("{} devices (ASSUMED - no LV docs)", sys.replace('_', " ")),
                quantity: 1.0,
                unit: "LOT".into(),
                assumed: true,
                scope: "ours".into(),
                spec_id: None,
            });
        }
        return lines;
    }

    // FA devices by kind.
    let mut fa_counts: BTreeMap<String, usize> = BTreeMap::new();
    for fa in &truth.fire_alarm_devices {
        *fa_counts.entry(fa.kind.clone()).or_insert(0) += 1;
    }
    for (kind, count) in &fa_counts {
        lines.push(LvBomEntry {
            system: "fire_alarm".into(),
            category: "device".into(),
            description: kind.replace('_', " "),
            quantity: *count as f64,
            unit: "EA".into(),
            assumed: false,
            scope: "ours".into(),
            spec_id: None,
        });
    }

    // Telecom devices by system + kind.
    let mut tc_counts: BTreeMap<(String, String), usize> = BTreeMap::new();
    for tc in &truth.telecom_devices {
        *tc_counts.entry((tc.lv_system.clone(), tc.kind.clone())).or_insert(0) += 1;
    }
    for ((sys, kind), count) in &tc_counts {
        let scope = if matches!(kind.as_str(), "idf" | "mdf" | "rack" | "backboard" | "patch_panel") { "lv_vendor" } else { "ours" };
        lines.push(LvBomEntry {
            system: sys.clone(),
            category: "device".into(),
            description: kind.replace('_', " "),
            quantity: *count as f64,
            unit: "EA".into(),
            assumed: false,
            scope: scope.into(),
            spec_id: None,
        });
    }

    // Cable footage.
    let mut cable_ft: BTreeMap<(String, String), f64> = BTreeMap::new();
    for fa in &truth.fire_alarm_devices {
        let route = estimate_route_ft(truth, &fa.level, [fa.at_ft[0], fa.at_ft[1]]) + 18.0;
        for c in fa_cables(fa) {
            *cable_ft.entry(("fire_alarm".into(), c)).or_insert(0.0) += route;
        }
    }
    for tc in &truth.telecom_devices {
        if matches!(tc.kind.as_str(), "idf" | "mdf" | "patch_panel" | "backboard" | "rack") {
            continue;
        }
        let route = estimate_route_ft(truth, &tc.level, [tc.at_ft[0], tc.at_ft[1]]) + 18.0;
        for c in tc_cables(tc) {
            *cable_ft.entry((tc.lv_system.clone(), c)).or_insert(0.0) += route;
        }
    }
    for ((sys, cable), ft) in &cable_ft {
        lines.push(LvBomEntry {
            system: sys.clone(),
            category: "cable".into(),
            description: cable.clone(),
            quantity: *ft,
            unit: "LF".into(),
            assumed: false,
            scope: "ours".into(),
            spec_id: None,
        });
    }

    // Supports.
    for cp in &truth.cable_pathways {
        if cp.kind != "j_hook" {
            continue;
        }
        let dx = cp.to_ft[0] - cp.from_ft[0];
        let dy = cp.to_ft[1] - cp.from_ft[1];
        let len = (dx * dx + dy * dy).sqrt();
        let spacing = cp.spacing_in.unwrap_or(48.0) / 12.0;
        lines.push(LvBomEntry {
            system: cp.systems.first().cloned().unwrap_or_default(),
            category: "support".into(),
            description: "J-hook support".into(),
            quantity: (len / spacing).ceil().max(1.0),
            unit: "EA".into(),
            assumed: false,
            scope: "ours".into(),
            spec_id: None,
        });
    }

    // Sleeves.
    let mut sl_counts: BTreeMap<(String, String), usize> = BTreeMap::new();
    for sl in &truth.sleeves {
        *sl_counts.entry((sl.material.clone(), format!("{}\"", sl.size_in))).or_insert(0) += 1;
    }
    for ((mat, sz), ct) in &sl_counts {
        lines.push(LvBomEntry {
            system: "general".into(),
            category: "sleeve".into(),
            description: format!("{} sleeve {}", mat, sz),
            quantity: *ct as f64,
            unit: "EA".into(),
            assumed: false,
            scope: "ours".into(),
            spec_id: None,
        });
    }

    // Firestop.
    let fs = truth.sleeves.iter().filter(|s| !s.fire_rating.is_empty()).count();
    if fs > 0 {
        lines.push(LvBomEntry {
            system: "general".into(),
            category: "firestop".into(),
            description: "Firestop system".into(),
            quantity: fs as f64,
            unit: "EA".into(),
            assumed: false,
            scope: "ours".into(),
            spec_id: None,
        });
    }
    lines
}

// ---- Sleeve layout sheet drawing (MUST 5) ---------------------------------------------------------

use cadaddy_doc::{HAlign, VAlign};
use cadaddy_geom::Vec2;

pub fn draw_sleeve_layout(pen: &mut crate::draw::Pen<'_>, t: &Truth, st: &crate::style::Style, level: &str) {
    let rot = crate::draw::readable(pen.rot());
    let th = st.text_in / pen.sh.scale;

    let system_layer = |systems: &[String]| -> &str {
        if systems.iter().any(|s| s == "fire_alarm") {
            "SL-FA"
        } else if systems.iter().any(|s| s == "security" || s == "access_control") {
            "SL-SEC"
        } else {
            "SL-DATA"
        }
    };

    let level_sleeves: Vec<&Sleeve> = t.sleeves.iter().filter(|s| s.level == level).collect();
    for sl in &level_sleeves {
        let c = pen.b([sl.at_ft[0], sl.at_ft[1]]);
        let r = th * 0.6;
        let layer = system_layer(&sl.systems);
        if sl.penetrates == "floor" {
            pen.circle(layer, c, r);
            let hs = r * 0.7;
            pen.fill(
                layer,
                [Vec2::new(c.x - hs, c.y - hs), Vec2::new(c.x + hs, c.y - hs), Vec2::new(c.x + hs, c.y + hs), Vec2::new(c.x - hs, c.y + hs)],
            );
        } else {
            pen.circle(layer, c, r);
        }
        let label = format!("{}\" {}", sl.size_in, sl.material);
        pen.text("SL-TEXT", Vec2::new(c.x + r * 1.5, c.y), st.text_in * 0.6, &label, rot, HAlign::Left, VAlign::Middle);
        if !sl.fire_rating.is_empty() {
            pen.text(
                "SL-TEXT",
                Vec2::new(c.x + r * 1.5, c.y - th * 0.8),
                st.text_in * 0.5,
                &format!("{} RATED + FS", sl.fire_rating),
                rot,
                HAlign::Left,
                VAlign::Top,
            );
        }
        let dim_text = format!("{:.1}', {:.1}'", sl.at_ft[0], sl.at_ft[1]);
        pen.text("SL-DIM", Vec2::new(c.x, c.y + r * 1.5), st.text_in * 0.5, &dim_text, rot, HAlign::Center, VAlign::Bottom);
    }

    // Sleeve schedule.
    if !level_sleeves.is_empty() {
        let fr = crate::layout::frame(st);
        let tx = fr.area[0][0] + 0.3;
        let mut ty = fr.area[0][1] + 0.5;
        let rh = st.text_in * 1.5;
        pen.text("SL-TEXT", pen.p([tx, ty]), st.text_in * 0.8, "SLEEVE SCHEDULE", rot, HAlign::Left, VAlign::Bottom);
        ty -= rh;
        for (dx, hdr) in [(0.0, "ID"), (1.0, "SIZE"), (2.0, "MATERIAL"), (3.2, "SYSTEM"), (4.5, "RATING")] {
            pen.text("SL-TEXT", pen.p([tx + dx, ty]), st.text_in * 0.6, hdr, rot, HAlign::Left, VAlign::Bottom);
        }
        for sl in level_sleeves.iter().take(40) {
            ty -= rh;
            let vals = [
                (0.0, sl.id.as_str()),
                (1.0, &format!("{}\"", sl.size_in)),
                (2.0, &sl.material),
                (3.2, &sl.systems.join(", ")),
                (4.5, &sl.fire_rating),
            ];
            for (dx, val) in &vals {
                pen.text("SL-TEXT", pen.p([tx + dx, ty]), st.text_in * 0.5, val, rot, HAlign::Left, VAlign::Bottom);
            }
        }
    }
}

// ---- Helpers --------------------------------------------------------------------------------------

fn room_map(truth: &Truth) -> BTreeMap<String, Vec<&Room>> {
    let mut m: BTreeMap<String, Vec<&Room>> = BTreeMap::new();
    for r in &truth.building.rooms {
        m.entry(r.level.clone()).or_default().push(r);
    }
    m
}

fn find_room(rooms: &BTreeMap<String, Vec<&Room>>, level: &str, pt: [f64; 2]) -> String {
    if let Some(rs) = rooms.get(level) {
        for r in rs {
            if pt[0] >= r.min_ft[0] && pt[0] <= r.max_ft[0] && pt[1] >= r.min_ft[1] && pt[1] <= r.max_ft[1] {
                return r.name.clone();
            }
        }
    }
    String::new()
}

fn find_classified_text(texts: &[([f64; 2], String)], pos: [f64; 2], tol: f64, classify: fn(&str) -> Option<&'static str>) -> Option<&'static str> {
    let mut best: Option<&'static str> = None;
    let mut best_dist = f64::MAX;
    for (tp, txt) in texts {
        let d = ((pos[0] - tp[0]).powi(2) + (pos[1] - tp[1]).powi(2)).sqrt();
        if d < tol
            && d < best_dist
            && let Some(k) = classify(txt)
        {
            best = Some(k);
            best_dist = d;
        }
    }
    best
}

fn find_candela_note(texts: &[([f64; 2], String)], pos: [f64; 2], tol: f64) -> Option<u32> {
    for (tp, txt) in texts {
        if ((pos[0] - tp[0]).powi(2) + (pos[1] - tp[1]).powi(2)).sqrt() > tol {
            continue;
        }
        let u = txt.to_uppercase();
        if u.contains("CD") {
            let num: String = u.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(n) = num.parse::<u32>()
                && [15, 30, 75, 110, 177].contains(&n)
            {
                return Some(n);
            }
        }
    }
    None
}

fn find_circuit_note(texts: &[([f64; 2], String)], pos: [f64; 2], tol: f64, prefix: &str) -> String {
    for (tp, txt) in texts {
        if ((pos[0] - tp[0]).powi(2) + (pos[1] - tp[1]).powi(2)).sqrt() > tol {
            continue;
        }
        if txt.to_uppercase().starts_with(prefix) {
            return txt.clone();
        }
    }
    String::new()
}

fn find_height_note(texts: &[([f64; 2], String)], pos: [f64; 2], tol: f64) -> Option<(f64, String)> {
    for (tp, txt) in texts {
        if ((pos[0] - tp[0]).powi(2) + (pos[1] - tp[1]).powi(2)).sqrt() > tol {
            continue;
        }
        let upper = txt.to_uppercase();
        if upper.contains("AFF")
            && let Some(h) = parse_height(&upper)
        {
            return Some((h, "note".into()));
        }
    }
    None
}

fn parse_height(text: &str) -> Option<f64> {
    let c = text.replace("AFF", "").replace("A.F.F.", "");
    let c = c.trim();
    if c.contains('\'') {
        let p: Vec<&str> = c.split('\'').collect();
        if p.len() >= 2 {
            let ft: f64 = p[0].trim().parse().unwrap_or(0.0);
            let inn: String = p[1].chars().filter(|ch| ch.is_ascii_digit() || *ch == '.').collect();
            return Some(ft * 12.0 + inn.parse::<f64>().unwrap_or(0.0));
        }
    }
    let num: String = c.chars().filter(|ch| ch.is_ascii_digit() || *ch == '.').collect();
    num.parse::<f64>().ok()
}

fn find_tag_note(
    texts: &[([f64; 2], String)],
    pos: [f64; 2],
    tol: f64,
    fa_class: fn(&str) -> Option<&'static str>,
    tc_class: fn(&str) -> Option<&'static str>,
) -> Option<String> {
    let mut best = None;
    let mut best_d = f64::MAX;
    for (tp, txt) in texts {
        let d = ((pos[0] - tp[0]).powi(2) + (pos[1] - tp[1]).powi(2)).sqrt();
        if d > tol || d >= best_d {
            continue;
        }
        let t = txt.trim();
        if t.len() > 1
            && t.len() <= 20
            && fa_class(t).is_none()
            && tc_class(t).is_none()
            && !t.to_uppercase().contains("AFF")
            && !t.to_uppercase().contains(" CD")
            && !t.to_uppercase().starts_with("SLC")
            && !t.to_uppercase().starts_with("NAC")
        {
            best = Some(t.to_string());
            best_d = d;
        }
    }
    best
}

fn find_cable_note(texts: &[([f64; 2], String)], pos: [f64; 2], tol: f64) -> Option<String> {
    for (tp, txt) in texts {
        if ((pos[0] - tp[0]).powi(2) + (pos[1] - tp[1]).powi(2)).sqrt() > tol {
            continue;
        }
        let u = txt.to_uppercase();
        if u.contains("CAT") || u.contains("RG") || u.contains("FIBER") {
            return Some(txt.clone());
        }
    }
    None
}

fn find_nearby_text(texts: &[([f64; 2], String)], pos: [f64; 2], tol: f64, kw: &[&str]) -> Option<String> {
    for (tp, txt) in texts {
        if ((pos[0] - tp[0]).powi(2) + (pos[1] - tp[1]).powi(2)).sqrt() > tol {
            continue;
        }
        let u = txt.to_uppercase();
        if kw.iter().any(|k| u.contains(k)) {
            return Some(txt.clone());
        }
    }
    None
}

fn lv_system_for(kind: &str) -> &'static str {
    match kind {
        "data_outlet" | "wifi_ap" | "patch_panel" | "idf" | "mdf" | "backboard" | "rack" => "data",
        "tv_outlet" => "tv",
        "telephone" => "telephone",
        "camera" | "card_reader" | "rex" | "mag_lock" | "door_contact" | "door_ajar" => "security",
        "nurse_call_station" | "nurse_call_display" | "ada_pull_cord" => "nurse_call",
        "speaker" => "sound",
        "pool_alarm" | "door_alarm" => "security",
        _ => "data",
    }
}

fn fa_cables(fa: &FireAlarmDevice) -> Vec<String> {
    match fa.kind.as_str() {
        "smoke_detector" | "heat_detector" | "co_detector" | "duct_detector" | "pull_station" | "monitor_module" | "relay_module"
        | "elevator_recall" | "door_holder" | "mag_door_holder" => vec!["1 x FPL".into()],
        "horn_strobe" | "speaker_strobe" | "strobe" | "speaker" | "nac_panel" => vec!["1 x FPL-NAC".into()],
        "facp" | "annunciator" => vec!["2 x FPL".into()],
        _ => vec!["1 x FPL".into()],
    }
}

fn tc_cables(tc: &TelecomDevice) -> Vec<String> {
    if let Some(ref s) = tc.cable_spec {
        return vec![s.clone()];
    }
    match tc.kind.as_str() {
        "data_outlet" | "wifi_ap" => vec!["2 x Cat6".into()],
        "tv_outlet" => vec!["1 x RG6".into()],
        _ => vec!["1 x Cat6".into()],
    }
}

fn seg_intersect(a1: [f64; 2], a2: [f64; 2], b1: [f64; 2], b2: [f64; 2]) -> Option<[f64; 2]> {
    let (dx1, dy1) = (a2[0] - a1[0], a2[1] - a1[1]);
    let (dx2, dy2) = (b2[0] - b1[0], b2[1] - b1[1]);
    let d = dx1 * dy2 - dy1 * dx2;
    if d.abs() < 1e-10 {
        return None;
    }
    let t = ((b1[0] - a1[0]) * dy2 - (b1[1] - a1[1]) * dx2) / d;
    let u = ((b1[0] - a1[0]) * dy1 - (b1[1] - a1[1]) * dx1) / d;
    if (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u) { Some([a1[0] + t * dx1, a1[1] + t * dy1]) } else { None }
}

fn level_height_ft(truth: &Truth, level: &str) -> f64 {
    let ls = &truth.building.levels;
    for (i, l) in ls.iter().enumerate() {
        if l.code == level || l.name.contains(level) {
            if let Some(next) = ls.get(i + 1) {
                return next.elevation_ft - l.elevation_ft;
            }
            return 10.0;
        }
    }
    10.0
}

fn estimate_route_ft(truth: &Truth, level: &str, pos: [f64; 2]) -> f64 {
    let mut min = f64::MAX;
    for tc in &truth.telecom_devices {
        if tc.level != level || !matches!(tc.kind.as_str(), "idf" | "mdf" | "patch_panel") {
            continue;
        }
        let d = (pos[0] - tc.at_ft[0]).abs() + (pos[1] - tc.at_ft[1]).abs();
        if d < min {
            min = d;
        }
    }
    if min < f64::MAX { min } else { 75.0 }
}
