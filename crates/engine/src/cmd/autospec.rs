//! `bspec.draft`: a first building spec from the plan alone, without any AI, from what the
//! perception tools see (`plan.layers`, `plan.walls`, `plan.openings`, `plan.rooms`,
//! `plan.notes`). Layer roles come from standard layer names, in two conventions (the US
//! National CAD Standard's `A-WALL`, `A-DOOR`, `A-GLAZ`, `E-LITE`…, and plain names such as
//! `Walls`, `Doors`, `Windows`, `Lighting`), and else from what a layer holds (wall face pairs,
//! door swings, room names). Wall types take their thickness from the measured face pairs; rooms
//! are the closed outlines round their name tags, with their numbers and any ceiling note inside;
//! door and window families are the blocks found as openings. Everything that is a convention or
//! a default rather than read from the plan is marked in the spec's `guess` (part key → why), so
//! the Plans to Model palette and agents know what to confirm.

use std::collections::BTreeMap;
use std::sync::Arc;

use cadaddy_doc::Entity;
use cadaddy_geom::Vec2;
use cadaddy_walk3d::{BuildingSpec, Family, FamilyKind, LayerRole, Level, Role, SPEC_VERSION, SpecRoom as Room, WallFrom, WallType, inches};
use serde_json::{Value, json};

use super::bspec::{part_json, plan_objects};
use super::perceive::geometry::{Limits, openings, pairs, room_number, segments};
use super::perceive::{feet_inches, texts, units_per_inch};
use super::*;
use crate::{Result, Session};

/// Layers looked at, at most (hostile drawings).
const MAX_LAYERS: usize = 2_000;
/// Rooms traced, at most.
const MAX_ROOMS: usize = 1_000;

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec::new("bspec.draft", "Draft Building Spec", run_draft).params(
        "{store?: bool (keep it as the drawing's spec; default false), levels?: [{name, elevation_ft, height_ft, ceiling_ft?, region?: [x0,y0,x1,y1], base?: [x,y], guess?: why its heights are assumed}] (the plans' levels, side by side on the sheet: only what lies in a level's region is read, and each room takes the level its tag is in; default: a level per plan view when the sheet prints several floors' plans (`3RD FLOOR PLAN`, `4TH FLOOR PLAN`), each over its view, heights assumed; else one level for the whole drawing), stack?: bool (build the levels stacked into one building)} → {spec, guesses: [{key, why}], parts, stored}: a first spec from the plan alone (standard layer names, wall face pairs, door blocks, room tags, ceiling notes, default heights), every guessed part listed in `guesses` and in the spec's `guess`",
    )]
}

/// A layer's role from its name: the US National CAD Standard's `D-MAJR-MINR` names, or plain
/// words. The convention is said in the reason.
pub(crate) fn role_by_name(layer: &str) -> Option<(Role, &'static str)> {
    let up = layer.to_ascii_uppercase();
    let b = up.as_bytes();
    // NCS: a discipline letter (or two), a dash, a four-letter major group.
    let ncs = b.len() >= 6
        && b.iter().take_while(|c| c.is_ascii_alphabetic()).count() <= 2
        && up.split('-').nth(1).is_some_and(|m| m.len() == 4 && m.chars().all(|c| c.is_ascii_alphabetic()));
    if ncs {
        let parts: Vec<&str> = up.split('-').collect();
        let major = parts.get(1).copied().unwrap_or("");
        let minor = parts.get(2).copied().unwrap_or("");
        let disc = parts.first().copied().unwrap_or("");
        let annotation = ["IDEN", "DIMS", "NOTE", "TEXT", "PATT", "HTCH", "SYMB", "NPLT"];
        let role = match (disc, major, minor) {
            (_, "AREA", "IDEN") | (_, "ANNO", "RMNO") | (_, "ANNO", "ROOM") => Some(Role::RoomTag),
            (_, "IDEN", _) if parts.len() == 2 => Some(Role::RoomTag),
            (_, "WALL", m) | (_, "GLAZ", m) | (_, "DOOR", m) if annotation.contains(&m) => None,
            (_, "GLAZ", "CWMG" | "CURT") | (_, "CWMG", _) => Some(Role::CurtainWall),
            (_, "WALL", _) => Some(Role::Wall),
            (_, "DOOR", _) => Some(Role::Door),
            (_, "GLAZ" | "WIND" | "WNDW", _) => Some(Role::Window),
            (_, "COLS", _) => Some(Role::Column),
            (_, "STRS" | "STAI", _) | (_, "FLOR", "STRS") => Some(Role::Stair),
            (_, "CLNG", "GRID") => Some(Role::CeilingGrid),
            ("E", "LITE", _) | ("P", "FIXT", _) | (_, "FLOR", "PFIX") => Some(Role::Fixture),
            ("E", "POWR" | "COMM" | "DATA" | "FIRE" | "SECU" | "DEVC", _) => Some(Role::Device),
            _ => None,
        };
        return role.map(|r| (r, "NCS layer name"));
    }
    let words: Vec<String> = up.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_string).collect();
    let has = |w: &[&str]| words.iter().any(|x| w.contains(&x.as_str()));
    let annotation = has(&["DIM", "DIMS", "DIMENSIONS", "HATCH", "TEXT", "NOTES"]);
    let role = if has(&["CURTAIN"]) {
        Some(Role::CurtainWall)
    } else if has(&["ROOM", "ROOMS"]) && has(&["NAME", "NAMES", "TAG", "TAGS", "NUMBER", "NUMBERS", "ID"]) || has(&["ROOMTAGS", "ROOMNAMES"]) {
        Some(Role::RoomTag)
    } else if annotation {
        None
    } else if has(&["WALL", "WALLS"]) {
        Some(Role::Wall)
    } else if has(&["DOOR", "DOORS"]) {
        Some(Role::Door)
    } else if has(&["WINDOW", "WINDOWS", "GLAZING"]) {
        Some(Role::Window)
    } else if has(&["COLUMN", "COLUMNS"]) {
        Some(Role::Column)
    } else if has(&["STAIR", "STAIRS", "STAIRCASE"]) {
        Some(Role::Stair)
    } else if has(&["CEILING", "RCP"]) && has(&["GRID"]) {
        Some(Role::CeilingGrid)
    } else if has(&["LIGHT", "LIGHTS", "LIGHTING", "LUMINAIRES", "FIXTURES"]) {
        Some(Role::Fixture)
    } else if has(&["OUTLETS", "RECEPTACLES", "DEVICES", "SWITCHES"]) {
        Some(Role::Device)
    } else {
        None
    };
    role.map(|r| (r, "plain layer name"))
}

/// A name text (not a number): two letters or more, at most 40 characters.
fn room_name(t: &str) -> bool {
    t.chars().filter(|c| c.is_alphabetic()).count() >= 2 && t.chars().count() <= 40 && !room_number(t)
}

/// A ceiling height (feet) from a note (`CLG 9'-0"`, `CEILING AT 8'6"`).
fn ceiling_note(t: &str) -> Option<f64> {
    let up = t.to_ascii_uppercase();
    if !(up.contains("CLG") || up.contains("CEILING")) {
        return None;
    }
    feet_inches(&up).map(|i| i / 12.0).filter(|f| (6.0..=40.0).contains(f))
}

fn median(mut v: Vec<f64>) -> Option<f64> {
    v.retain(|x| x.is_finite());
    v.sort_by(f64::total_cmp);
    v.get(v.len() / 2).copied()
}

/// Levels given to `bspec.draft` (most; hostile params).
const MAX_LEVELS: usize = 200;

/// The plans' levels given to `bspec.draft` (`levels`), with why each one's heights are guessed.
fn given_levels(p: &Value) -> Result<Vec<(Level, Option<String>)>> {
    let Some(list) = p.get("levels").filter(|v| !v.is_null()) else { return Ok(Vec::new()) };
    let list = list.as_array().ok_or_else(|| bad("bspec.draft", "`levels` is a list of {name, elevation_ft, height_ft, region?}"))?;
    if list.len() > MAX_LEVELS {
        return Err(bad("bspec.draft", format!("`levels` holds at most {MAX_LEVELS} levels")));
    }
    let mut out: Vec<(Level, Option<String>)> = Vec::new();
    for (i, v) in list.iter().enumerate() {
        let guess = v.get("guess").and_then(Value::as_str).map(|g| g.chars().take(240).collect::<String>());
        let mut v = v.clone();
        if let Some(o) = v.as_object_mut() {
            o.remove("guess");
        }
        let l: Level = serde_json::from_value(v).map_err(|e| bad("bspec.draft", format!("levels[{i}]: {e}")))?;
        let finite = [l.elevation_ft, l.height_ft, l.ceiling_ft].iter().all(|x| x.is_finite())
            && l.region.is_none_or(|r| r.iter().all(|x| x.is_finite()))
            && l.base.is_none_or(|b| b.iter().all(|x| x.is_finite()));
        if l.name.trim().is_empty() || !finite || l.height_ft <= 0.0 {
            return Err(bad("bspec.draft", format!("levels[{i}] needs a name, finite elevation_ft and a height_ft above 0")));
        }
        if out.iter().any(|(o, _)| o.name.eq_ignore_ascii_case(&l.name)) {
            return Err(bad("bspec.draft", format!("levels[{i}]: {} is given twice", l.name)));
        }
        out.push((l, guess));
    }
    Ok(out)
}

/// Floor to floor and ceiling heights of a level read off a plan view's title, feet (assumed).
const VIEW_FLOOR_FT: f64 = 10.0;
const VIEW_CEILING_FT: f64 = 8.0;

/// One level per plan view of a sheet that prints several, each of its own level (`3RD FLOOR
/// PLAN` and `4TH FLOOR PLAN` on one sheet): its region the view's cell on the sheet, its
/// elevation from the floor number, floor to floor assumed. Without them the views would build
/// side by side as one storey. Empty for a sheet of one view.
fn view_levels(d: &cadaddy_doc::Drawing) -> Vec<(Level, Option<String>)> {
    let ext = d.extents(&cadaddy_doc::Space::Model);
    if ext.is_empty() || !(ext.width().is_finite() && ext.height().is_finite()) {
        return Vec::new();
    }
    let pad = 0.01 * ext.width().max(ext.height());
    let (lo, hi) = ([ext.min.x - pad, ext.min.y - pad], [ext.max.x + pad, ext.max.y + pad]);
    let mut out: Vec<(Level, Option<String>)> = Vec::new();
    for (v, [x0, y0, x1, y1]) in cadaddy_mepfix::sheet_reader::view_cells(d) {
        // A view of one storey: `3`, or a basement `B1` (a typical plan of several levels, a
        // roof plan, are not one storey's plan).
        let [code] = v.levels.as_slice() else { continue };
        let floor: Option<f64> = match code.strip_prefix('B') {
            Some(b) => b.parse::<u32>().ok().filter(|n| *n >= 1).map(|n| -f64::from(n)),
            None => code.parse::<u32>().ok().filter(|n| *n >= 1).map(|n| f64::from(n) - 1.0),
        };
        let Some(floor) = floor else { continue };
        let name = format!("Level {code}");
        if out.iter().any(|(l, _)| l.name == name) {
            continue;
        }
        let region = [x0.max(lo[0]), y0.max(lo[1]), x1.min(hi[0]), y1.min(hi[1])];
        if !region.iter().all(|x| x.is_finite()) || region[2] <= region[0] || region[3] <= region[1] {
            continue;
        }
        let title: String = v.title.chars().take(80).collect();
        let why = format!(
            "plan view \"{title}\" on the sheet; floor to floor {} and ceiling {} assumed",
            cadaddy_walk3d::feet(VIEW_FLOOR_FT),
            cadaddy_walk3d::feet(VIEW_CEILING_FT)
        );
        let level = Level {
            name,
            elevation_ft: floor * VIEW_FLOOR_FT,
            height_ft: VIEW_FLOOR_FT,
            ceiling_ft: VIEW_CEILING_FT,
            region: Some(region),
            base: None,
            attr: None,
            sheet: None,
        };
        out.push((level, Some(why)));
    }
    if out.len() < 2 {
        return Vec::new();
    }
    out.sort_by(|a, b| a.0.elevation_ft.total_cmp(&b.0.elevation_ft));
    out
}

/// [`draft`], with layer roles from what a layer holds (`by_content`) or from layer names only.
/// `plan.classify` has already sorted a PDF sheet's linework by what it draws and put walls,
/// doors and room names on their own layers: what it left behind (a duct's sides, a stair's
/// treads) is not walls however many line pairs it holds.
pub(crate) fn draft_with(s: &Session, by_content: bool) -> Result<BuildingSpec> {
    draft_levels(s, by_content, Vec::new())
}

/// [`draft_with`] for the plans' `levels` (none: one level for the whole drawing). With levels
/// that have regions, only what lies in a region is read (the other sheets on the drawing, an
/// electrical plan's copy of the walls, add nothing) and each room takes its tag's level.
fn draft_levels(s: &Session, by_content: bool, given: Vec<(Level, Option<String>)>) -> Result<BuildingSpec> {
    let plan = super::verify::plan_session(s)?;
    let d = plan.doc()?;
    let given = if given.is_empty() { view_levels(d) } else { given };
    let mut ents = plan_objects(d);
    let given_levels: Vec<Level> = given.iter().map(|(l, _)| l.clone()).collect();
    let regions = !given_levels.is_empty() && given_levels.iter().all(|l| l.region.is_some());
    if regions {
        ents.retain(|e| super::bspec::centre(d, e).is_some_and(|c| cadaddy_walk3d::level_at(&given_levels, c.x, c.y).is_some()));
    }
    let k = units_per_inch(d);
    let mut guess: BTreeMap<String, String> = BTreeMap::new();
    // Layers with objects, in drawing order of their first object.
    let mut layers: Vec<String> = Vec::new();
    for e in &ents {
        if layers.len() >= MAX_LAYERS {
            break;
        }
        if !layers.iter().any(|l| l.eq_ignore_ascii_case(&e.common.layer)) {
            layers.push(e.common.layer.to_string());
        }
    }
    let lim = Limits { min_thick: 2.5 * k, max_thick: 20.0 * k, min_len: 12.0 * k };
    let on = |layer: &str| -> Vec<&Arc<Entity>> { ents.iter().copied().filter(|e| e.common.layer.eq_ignore_ascii_case(layer)).collect() };
    let all_texts = texts(&ents);
    let mut roles: Vec<LayerRole> = Vec::new();
    let mut wall_types: Vec<WallType> = Vec::new();
    for layer in &layers {
        let mine = on(layer);
        let segs = segments(&mine);
        let found = pairs(d, &segs, &lim);
        let by_name = role_by_name(layer);
        // Else what it holds: wall face pairs, door swings, room names.
        let by_content = || -> Option<(Role, String)> {
            if !by_content {
                return None;
            }
            if found.len() >= 2 {
                return Some((Role::Wall, format!("{} wall face pairs on it", found.len())));
            }
            let swings = openings(d, &mine, &segs, k).iter().filter(|o| o.kind == "door" && o.swing.is_some()).count();
            if swings >= 1 && swings * 2 >= mine.len().min(4) {
                return Some((Role::Door, format!("{swings} door swings on it")));
            }
            None
        };
        let (role, why) = match (by_name, by_content()) {
            (Some((r, conv)), _) => (r, format!("{conv} ({layer})")),
            (None, Some((r, why))) => (r, why),
            (None, None) => continue,
        };
        let mut lr = LayerRole { layer: layer.clone(), role, wall_type: None, height_ft: None };
        if role == Role::Wall {
            // The type: the median face spacing, else centrelines of a common partition.
            let (ty, measured) = match median(found.iter().map(|p| p.thickness / k).collect()) {
                Some(t) => {
                    let t = (t * 8.0).round() / 8.0;
                    (WallType { name: format!("{} walls", inches(t)), height_ft: None, thickness_in: t, from: WallFrom::Pairs, assembly: None }, true)
                }
                None => (
                    WallType { name: "Centreline walls".into(), height_ft: None, thickness_in: 4.875, from: WallFrom::Centerline, assembly: None },
                    false,
                ),
            };
            if !measured {
                guess.insert(format!("wall_types/{}", ty.name), "no face pairs: drawn as centrelines, a 4 7/8\" partition assumed".into());
            }
            lr.wall_type = Some(ty.name.clone());
            if !wall_types.iter().any(|t| t.name.eq_ignore_ascii_case(&ty.name)) {
                wall_types.push(ty);
            }
        }
        guess.insert(format!("layers/{layer}"), why);
        roles.push(lr);
    }
    // Room tags: the room tag layers' texts, else names on a layer with no other role (a
    // fixture's or a duct's label never names a room) that close an outline.
    let tagged: Vec<&str> = roles.iter().filter(|r| r.role == Role::RoomTag).map(|r| r.layer.as_str()).collect();
    let layer_of = |h: cadaddy_doc::Handle| ents.iter().find(|e| e.handle == h).map(|e| e.common.layer.as_str());
    let candidates: Vec<(cadaddy_doc::Handle, String, Vec2, f64)> = all_texts
        .iter()
        .filter(|(h, t, _, _)| {
            let layer = layer_of(*h).unwrap_or_default();
            room_name(t)
                && if tagged.is_empty() {
                    !roles.iter().any(|r| r.layer.eq_ignore_ascii_case(layer))
                } else {
                    tagged.iter().any(|l| l.eq_ignore_ascii_case(layer))
                }
        })
        .map(|(h, t, at, ht)| (*h, t.clone(), *at, *ht))
        .collect();
    // Traced on a plan of what bounds rooms (the wall, door and window layers just found, each
    // centreline wall as its two faces), so MEP lines, stairs and tags never split a room.
    let roles_spec = serde_json::from_value::<BuildingSpec>(json!({
        "units_per_foot": 12.0 * k,
        "layers": serde_json::to_value(&roles).unwrap_or_default(),
        "wall_types": serde_json::to_value(&wall_types).unwrap_or_default(),
    }))
    .ok();
    let bounds_plan = super::bspec::room_plan_for(&plan, roles_spec)?;
    let trace_on = bounds_plan.as_ref().unwrap_or(&plan);
    let ext = d.extents(&cadaddy_doc::Space::Model);
    let sheet = (ext.width() * ext.height()).max(1e-9);
    let numbers: Vec<&(cadaddy_doc::Handle, String, Vec2, f64)> = all_texts.iter().filter(|(_, t, _, _)| room_number(t)).collect();
    let notes: Vec<(f64, Vec2)> = all_texts.iter().filter_map(|(_, t, at, _)| ceiling_note(t).map(|c| (c, *at))).collect();
    let mut outlines: Vec<Vec<Vec2>> = Vec::new();
    let mut named: Vec<(usize, cadaddy_doc::Handle, String, Vec2, f64)> = Vec::new();
    let mut outline_cache = None;
    let max_rooms = if given_levels.is_empty() { MAX_ROOMS } else { MAX_ROOMS * given_levels.len().min(10) };
    for (h, name, at, height) in candidates.into_iter().take(max_rooms) {
        let Ok(loops) = super::hatch::loops_at_lazy(trace_on, at, &mut outline_cache) else { continue };
        let Some(outer) = loops.iter().find(|l| l.outer).or(loops.first()) else { continue };
        let poly = cadaddy_geom::Polyline { vertices: outer.vertices.clone(), closed: true }.tessellate(1e-3);
        let area = cadaddy_geom::shoelace(&poly).abs();
        if poly.len() < 3 || area < 16.0 * 144.0 * k * k || area > 0.5 * sheet {
            continue;
        }
        let i = match outlines.iter().position(|q| super::bspec::same_outline(q, &poly)) {
            Some(i) => i,
            None => {
                outlines.push(poly);
                outlines.len() - 1
            }
        };
        named.push((i, h, name, at, height));
    }
    let mut rooms: Vec<Room> = Vec::new();
    let mut room_layers: Vec<String> = Vec::new();
    for (i, h, name) in super::bspec::room_tags(&outlines, &named) {
        let (Some(poly), Some(at)) = (outlines.get(i), named.iter().find(|n| n.1 == h).map(|n| n.3)) else { continue };
        let poly = poly.clone();
        let number = numbers
            .iter()
            .filter(|(_, _, q, _)| cadaddy_geom::point_in_polygon(&poly, *q))
            .min_by(|a, b| a.2.dist(at).total_cmp(&b.2.dist(at)))
            .map(|(_, t, _, _)| t.clone());
        let ceiling = median(notes.iter().filter(|(_, q)| cadaddy_geom::point_in_polygon(&poly, *q)).map(|(c, _)| *c).collect());
        let title = name.split_whitespace().map(|w| {
            let mut c = w.chars();
            c.next().map_or(String::new(), |f| f.to_uppercase().chain(c.flat_map(char::to_lowercase)).collect())
        });
        let title: Vec<String> = title.collect();
        let room_name = title.join(" ");
        // On the plans' levels a room is its name, number and level (every guest room is a
        // KING); on one sheet, its name.
        let level = if given_levels.is_empty() { None } else { cadaddy_walk3d::level_at(&given_levels, at.x, at.y).map(|l| l.name.clone()) };
        if !given_levels.is_empty() && level.is_none() {
            continue;
        }
        let same = |r: &Room| {
            r.name.eq_ignore_ascii_case(&room_name)
                && (given_levels.is_empty()
                    || (r.number.as_deref().unwrap_or("").eq_ignore_ascii_case(number.as_deref().unwrap_or("")) && r.level == level))
        };
        if rooms.iter().any(same) {
            continue;
        }
        if let Some(e) = ents.iter().find(|e| e.handle == h)
            && !room_layers.iter().any(|l| l.eq_ignore_ascii_case(&e.common.layer))
        {
            room_layers.push(e.common.layer.to_string());
        }
        rooms.push(Room { name: room_name, number, level, ceiling_ft: ceiling, materials: Default::default() });
    }
    // Names found on a layer with no role make it a room tag layer.
    for l in &room_layers {
        if !roles.iter().any(|r| r.layer.eq_ignore_ascii_case(l)) {
            roles.push(LayerRole { layer: l.clone(), role: Role::RoomTag, wall_type: None, height_ft: None });
            guess.insert(format!("layers/{l}"), "room names on it close outlines".into());
        }
    }
    // One level, with the ceiling from the notes (else Walk mode's defaults).
    let note_ceiling = median(notes.iter().map(|(c, _)| *c).collect());
    let ceiling = note_ceiling.unwrap_or(8.0);
    let height = (ceiling + 2.0).max(10.0);
    let level = Level {
        name: "Level 1".into(),
        elevation_ft: 0.0,
        height_ft: height,
        ceiling_ft: ceiling,
        region: None,
        base: None,
        attr: None,
        sheet: None,
    };
    let levels = if given.is_empty() {
        guess.insert(
            "levels/Level 1".into(),
            match note_ceiling {
                Some(c) => format!("one level; ceiling {} from the notes, floor to floor assumed", cadaddy_walk3d::feet(c)),
                None => "one level; floor to floor and ceiling heights assumed".into(),
            },
        );
        vec![level]
    } else {
        for (l, why) in &given {
            if let Some(why) = why {
                guess.insert(format!("levels/{}", l.name), why.clone());
            }
        }
        given_levels.clone()
    };
    // Door and window families: the blocks found as openings, one family per block.
    let mut families: Vec<Family> = Vec::new();
    for o in openings(d, &ents, &segments(&ents), k) {
        let Some(block) = &o.block else { continue };
        let by_layer = roles.iter().find(|r| r.layer.eq_ignore_ascii_case(&o.layer)).map(|r| r.role);
        let kind = match by_layer {
            Some(Role::Window) => FamilyKind::Window,
            Some(Role::Door) => FamilyKind::Door,
            _ if o.kind == "window" => FamilyKind::Window,
            _ => FamilyKind::Door,
        };
        if families.iter().any(|f| f.block.eq_ignore_ascii_case(block)) || families.len() >= 1000 {
            continue;
        }
        let name = block.clone();
        guess.insert(format!("families/{name}"), "from the block's name; heights are the defaults".into());
        families.push(Family { name, kind, block: block.clone(), width_in: None, width_attr: None, head_ft: None, sill_ft: None, height_attr: None });
    }
    for r in &rooms {
        if r.ceiling_ft.is_none() {
            guess.insert(format!("rooms/{}", r.name), "no ceiling note in the room: the level's ceiling".into());
        }
    }
    guess.insert("openings".into(), "default door head and window sill and head".into());
    let mut sp = BuildingSpec {
        version: SPEC_VERSION,
        // Rounded: 12 × 25.4 is 304.79999… in binary.
        units_per_foot: (12.0 * k * 1e6).round() / 1e6,
        levels,
        layers: roles,
        wall_types,
        rooms,
        families,
        ..BuildingSpec::from_rules(&cadaddy_walk3d::Rules::fallback())
    };
    sp.devices = Default::default();
    sp.device_sheets = Vec::new();
    sp.approved = Vec::new();
    sp.guess = guess;
    Ok(sp)
}

fn run_draft(s: &mut Session, p: &Value) -> Result<Value> {
    let mut sp = draft_levels(s, true, given_levels(p)?)?;
    sp.stack = bool_or(p, "stack", false);
    let store = bool_or(p, "store", false);
    if store {
        s.doc_mut()?.building_spec = Some(sp.to_json());
    }
    let guesses: Vec<Value> = sp.guess.iter().map(|(k, why)| json!({ "key": k, "why": why })).collect();
    let parts: Vec<Value> = sp.describe().iter().map(part_json).collect();
    Ok(json!({ "spec": serde_json::to_value(&sp).unwrap_or(Value::Null), "guesses": guesses, "parts": parts, "stored": store }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layer_names_in_both_conventions() {
        let r = |l: &str| role_by_name(l).map(|x| x.0);
        assert_eq!(r("A-WALL"), Some(Role::Wall));
        assert_eq!(r("A-WALL-FULL"), Some(Role::Wall));
        assert_eq!(r("A-WALL-PATT"), None);
        assert_eq!(r("A-DOOR"), Some(Role::Door));
        assert_eq!(r("A-GLAZ"), Some(Role::Window));
        assert_eq!(r("A-GLAZ-CWMG"), Some(Role::CurtainWall));
        assert_eq!(r("S-COLS"), Some(Role::Column));
        assert_eq!(r("A-FLOR-STRS"), Some(Role::Stair));
        assert_eq!(r("A-AREA-IDEN"), Some(Role::RoomTag));
        assert_eq!(r("E-LITE-CLNG"), Some(Role::Fixture));
        assert_eq!(r("E-POWR-DEVC"), Some(Role::Device));
        assert_eq!(r("A-ANNO-DIMS"), None);
        assert_eq!(r("Walls"), Some(Role::Wall));
        assert_eq!(r("Interior Walls"), Some(Role::Wall));
        assert_eq!(r("Wall Hatch"), None);
        assert_eq!(r("DOORS"), Some(Role::Door));
        assert_eq!(r("Windows"), Some(Role::Window));
        assert_eq!(r("Curtain Wall"), Some(Role::CurtainWall));
        assert_eq!(r("Room Names"), Some(Role::RoomTag));
        assert_eq!(r("Lighting"), Some(Role::Fixture));
        assert_eq!(r("Furniture"), None);
        assert_eq!(r("0"), None);
        assert_eq!(ceiling_note("CLG 9'-0\""), Some(9.0));
        assert_eq!(ceiling_note("9'-0\""), None);
    }
}
