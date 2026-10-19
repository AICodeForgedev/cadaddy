//! Camera tours (`cadaddy-walk3d`'s [`Tours`], kept in the drawing): `tour.view` saves a named
//! viewpoint (an eye, a heading, a level, the room it shows), `tour.rooms` one per room on the
//! plans, `tour.create` a camera path through viewpoints (speed, where it looks, a pause at each),
//! `tour.list` lists them and `tour.timing` says how long a tour runs, how many frames an export
//! makes and where the camera is at a moment. Walk plays tours; `cadaddy-walk --tour` exports them.
//!
//! Positions are Walk's: feet, x east, y north, z up, as the building spec or the walk rules place
//! the plan.

use cadaddy_walk3d::{BuildingSpec, Look, Rules, Tour, Tours, Viewpoint, rooms};
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

/// Eye height above the floor for viewpoints placed in rooms, feet.
const EYE_FT: f64 = 5.5;
/// Longest name kept.
const MAX_NAME: usize = 120;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("tour.view", "Save Viewpoint", run_view).params("{name, eye?: [x, y, z] feet (Walk's coordinates), room?: room name on the plans (stand there, 5'6\" above its floor; the eye wins when both are given), yaw?: degrees from east toward north (0), pitch?: degrees up (0), level?: index, delete?: true} → {view, views: count}"),
        CommandSpec::new("tour.rooms", "Viewpoints for Every Room", run_rooms)
            .menu(&["View", "Walk and Fly", "Viewpoints for Every Room"])
            .params("{tour?: name (also make a tour through them, level by level), speed?, look?, pause?} → {added: [names], tour?}"),
        CommandSpec::new("tour.create", "Camera Tour", run_create).params("{name, stops: [viewpoint names], speed?: feet per second (4.5), look?: \"path\" (ahead, default) | \"views\" (turn between the viewpoints' headings) | [x, y, z] (always at that point), pause?: seconds at each stop (0), delete?: true} → {tour, length, duration}"),
        CommandSpec::new("tour.list", "Tours and Viewpoints", run_list).menu(&["View", "Walk and Fly", "Tours and Viewpoints"]).params("{} → {views: [viewpoint], tours: [{name, stops, speed, look, pause, length, duration}]}"),
        CommandSpec::new("tour.timing", "Tour Timing", run_timing).params(
            "{name, fps?: frames per second (30), at?: seconds} → {length (feet), duration (s), fps, frames (an export's frame count: one at the start, then every 1/fps s), pose?: {eye, yaw, pitch, level} at `at`}",
        ),
    ]
}

fn load(s: &Session, cmd: &str) -> Result<Tours> {
    Tours::from_json(s.doc()?.tours.as_deref()).map_err(|e| bad(cmd, e))
}

fn store(s: &mut Session, t: &Tours) -> Result<()> {
    s.doc_mut()?.tours = t.to_json();
    Ok(())
}

fn name_param(p: &Value, cmd: &str) -> Result<String> {
    let n = str_param(p, "name").map(str::trim).unwrap_or_default();
    if n.is_empty() {
        return Err(bad(cmd, "`name` is required"));
    }
    Ok(n.chars().take(MAX_NAME).collect())
}

fn num(p: &Value, key: &str) -> Option<f64> {
    p.get(key).and_then(Value::as_f64).filter(|v| v.is_finite())
}

fn xyz(v: &Value) -> Option<[f64; 3]> {
    let a = v.as_array()?;
    let c = |i: usize| a.get(i).and_then(Value::as_f64).filter(|v| v.is_finite());
    Some([c(0)?, c(1)?, c(2).unwrap_or(0.0)])
}

/// The rules placing the plan in Walk: the building spec's, else the walk rules.
fn walk_rules(s: &Session) -> Result<Rules> {
    if let Some(spec) = s.doc()?.building_spec.as_deref().and_then(|t| BuildingSpec::from_json(t).ok()) {
        return Ok(spec.to_rules());
    }
    super::wallgen::rules_for(s, &json!({}))
}

/// Rooms on the plan: (name, level, eye).
fn plan_rooms(s: &Session) -> Result<Vec<(String, usize, [f64; 3])>> {
    let rules = walk_rules(s)?;
    let d = s.doc()?;
    let found = rooms(&[d], &rules);
    Ok(found
        .into_iter()
        .map(|r| {
            let floor = rules.levels.get(r.level).map_or(0.0, |l| l.elevation_ft);
            (r.name, r.level, [r.at.x, r.at.y, floor + EYE_FT])
        })
        .collect())
}

fn view_json(v: &Viewpoint) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

fn run_view(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "tour.view";
    let name = name_param(p, CMD)?;
    let mut t = load(s, CMD)?;
    if bool_or(p, "delete", false) {
        let before = t.views.len();
        t.views.retain(|v| !v.name.eq_ignore_ascii_case(&name));
        if t.views.len() == before {
            return Err(bad(CMD, format!("no viewpoint named {name}")));
        }
        if let Some(tour) = t.tours.iter().find(|x| x.stops.iter().any(|st| st.eq_ignore_ascii_case(&name))) {
            return Err(bad(CMD, format!("tour {} passes through {name}; change or delete it first", tour.name)));
        }
        store(s, &t)?;
        return Ok(json!({"deleted": name, "views": t.views.len()}));
    }
    let room = str_param(p, "room").map(str::trim).filter(|r| !r.is_empty()).map(str::to_string);
    let at_room = match &room {
        Some(r) => plan_rooms(s)?.into_iter().find(|(n, ..)| n.eq_ignore_ascii_case(r)),
        None => None,
    };
    let eye = match (p.get("eye").and_then(xyz), &at_room) {
        (Some(e), _) => e,
        (None, Some((_, _, e))) => *e,
        (None, None) if room.is_some() => return Err(bad(CMD, format!("no room named {} on the plans", room.unwrap_or_default()))),
        (None, None) => return Err(bad(CMD, "give `eye` or `room`")),
    };
    let level = match p.get("level").and_then(Value::as_u64) {
        Some(l) => usize::try_from(l).unwrap_or(usize::MAX).min(10_000),
        None => at_room.as_ref().map_or(0, |r| r.1),
    };
    let v = Viewpoint {
        name,
        room: at_room.map(|r| r.0).or(room).unwrap_or_default().chars().take(MAX_NAME).collect(),
        level,
        eye,
        yaw: num(p, "yaw").unwrap_or(0.0).rem_euclid(360.0),
        pitch: num(p, "pitch").unwrap_or(0.0).clamp(-85.0, 85.0),
    };
    t.put_view(v.clone()).map_err(|e| bad(CMD, e))?;
    store(s, &t)?;
    Ok(json!({"view": view_json(&v), "views": t.views.len()}))
}

/// `look` from params: "path", "views" or a point.
fn look_param(p: &Value, cmd: &str) -> Result<Look> {
    match p.get("look") {
        None | Some(Value::Null) => Ok(Look::Path),
        Some(Value::String(s)) if s.eq_ignore_ascii_case("path") => Ok(Look::Path),
        Some(Value::String(s)) if s.eq_ignore_ascii_case("views") => Ok(Look::Views),
        Some(v) => xyz(v).map(Look::At).ok_or_else(|| bad(cmd, "`look` is \"path\", \"views\" or [x, y, z]")),
    }
}

fn tour_from(p: &Value, cmd: &str, name: String, stops: Vec<String>) -> Result<Tour> {
    Ok(Tour { name, stops, speed: num(p, "speed").unwrap_or(4.5), look: look_param(p, cmd)?, pause: num(p, "pause").unwrap_or(0.0) })
}

fn timing(t: &Tours, tour: &Tour) -> (f64, f64) {
    t.path(tour).map(|path| (path.length(), path.duration())).unwrap_or((0.0, 0.0))
}

fn run_create(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "tour.create";
    let name = name_param(p, CMD)?;
    let mut t = load(s, CMD)?;
    if bool_or(p, "delete", false) {
        let before = t.tours.len();
        t.tours.retain(|x| !x.name.eq_ignore_ascii_case(&name));
        if t.tours.len() == before {
            return Err(bad(CMD, format!("no tour named {name}")));
        }
        store(s, &t)?;
        return Ok(json!({"deleted": name, "tours": t.tours.len()}));
    }
    let stops: Vec<String> = p
        .get("stops")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).take(cadaddy_walk3d::MAX_TOUR_STOPS + 1).map(str::to_string).collect())
        .unwrap_or_default();
    if stops.is_empty() {
        return Err(bad(CMD, "`stops` lists the viewpoints the tour passes through"));
    }
    let tour = tour_from(p, CMD, name, stops)?;
    t.put_tour(tour.clone()).map_err(|e| bad(CMD, e))?;
    let (length, duration) = timing(&t, &tour);
    store(s, &t)?;
    Ok(json!({"tour": serde_json::to_value(&tour).unwrap_or(Value::Null), "length": length, "duration": duration}))
}

fn run_rooms(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "tour.rooms";
    let mut t = load(s, CMD)?;
    let mut found = plan_rooms(s)?;
    if found.is_empty() {
        return Err(crate::EngineError::Other("No room names on the plans.".into()));
    }
    found.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    let mut added = Vec::new();
    for (room, level, eye) in found {
        // Rooms of the same name on several levels: the name gets the level.
        let name: String =
            if added.iter().any(|n: &String| n.eq_ignore_ascii_case(&room)) { format!("{room} ({})", level + 1) } else { room.clone() };
        let v = Viewpoint { name: name.chars().take(MAX_NAME).collect(), room, level, eye, yaw: 0.0, pitch: 0.0 };
        if t.put_view(v.clone()).is_ok() {
            added.push(v.name);
        }
    }
    let mut out = json!({"added": added});
    if let Some(tn) = str_param(p, "tour").map(str::trim).filter(|n| !n.is_empty()) {
        let tour = tour_from(p, CMD, tn.chars().take(MAX_NAME).collect(), added.clone())?;
        t.put_tour(tour.clone()).map_err(|e| bad(CMD, e))?;
        let (length, duration) = timing(&t, &tour);
        out["tour"] = json!({"name": tour.name, "length": length, "duration": duration});
    }
    store(s, &t)?;
    Ok(out)
}

fn run_list(s: &mut Session, _p: &Value) -> Result<Value> {
    let t = load(s, "tour.list")?;
    let tours: Vec<Value> = t
        .tours
        .iter()
        .map(|x| {
            let (length, duration) = timing(&t, x);
            let mut v = serde_json::to_value(x).unwrap_or(Value::Null);
            v["length"] = json!(length);
            v["duration"] = json!(duration);
            v
        })
        .collect();
    Ok(json!({"views": t.views.iter().map(view_json).collect::<Vec<_>>(), "tours": tours}))
}

fn run_timing(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "tour.timing";
    let name = name_param(p, CMD)?;
    let t = load(s, CMD)?;
    let tour = t.tour(&name).ok_or_else(|| bad(CMD, format!("no tour named {name}")))?;
    let path = t.path(tour).map_err(|e| bad(CMD, e))?;
    let fps = num(p, "fps").unwrap_or(30.0);
    if !(0.1..=240.0).contains(&fps) {
        return Err(bad(CMD, "`fps` is between 0.1 and 240"));
    }
    let mut out = json!({"tour": tour.name, "length": path.length(), "duration": path.duration(), "fps": fps, "frames": path.frame_count(fps)});
    if let Some(at) = num(p, "at") {
        let q = path.pose(at);
        out["pose"] = json!({"eye": [q.eye.x, q.eye.y, q.eye.z], "yaw": q.yaw.to_degrees(), "pitch": q.pitch.to_degrees(), "level": q.level});
    }
    Ok(out)
}
