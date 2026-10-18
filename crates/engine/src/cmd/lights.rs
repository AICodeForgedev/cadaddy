//! Lights of rendered views: LIGHT (and POINTLIGHT, SPOTLIGHT, DISTANTLIGHT), light.set and
//! LIGHTLIST; the sun (SUNPROPERTIES, SUNSTATUS) and the exposure of rendered views
//! (RENDEREXPOSURE). The model is [`cadaddy_doc::lighting`]; the Realistic visual style draws
//! with them ([`cadaddy_render::light`]).

use cadaddy_doc::lighting::{Attenuation, Light, LightKind, Sun};
use cadaddy_doc::{EntityKind, Handle};
use cadaddy_geom::{Vec2, Vec3};
use serde_json::{Value, json};

use super::*;
use crate::{Accept, Input, Interactive, Prompt, Result, Session, Step};

pub fn specs() -> Vec<CommandSpec> {
    let light_params = "{kind?: point|spot|distant (default point), position: [x, y, z?], target?: [x, y, z?] (spot and distant: where it points; default straight down), intensity? (candela; distant: a factor of the sun), color?: colour index 1–255, name?, on?, shadows?, hotspot?, falloff? (spot, degrees), attenuation?: none|linear|square} → {handle}";
    vec![
        CommandSpec::new("light", "Light", run_light)
            .menu(&["View", "Render", "Light"])
            .params(light_params)
            .interactive(|_| Ok(Box::new(LightM::new(None)))),
        CommandSpec::new("pointlight", "Point Light", |s, p| run_kind(s, p, LightKind::Point))
            .params("as light, kind point")
            .interactive(|_| Ok(Box::new(LightM::new(Some(LightKind::Point))))),
        CommandSpec::new("spotlight", "Spotlight", |s, p| run_kind(s, p, LightKind::Spot))
            .params("as light, kind spot")
            .interactive(|_| Ok(Box::new(LightM::new(Some(LightKind::Spot))))),
        CommandSpec::new("distantlight", "Distant Light", |s, p| run_kind(s, p, LightKind::Distant))
            .params("as light, kind distant")
            .interactive(|_| Ok(Box::new(LightM::new(Some(LightKind::Distant))))),
        CommandSpec::new("light.set", "Change Lights", run_set).params("{handles?: [hex] (default: the selection), …the fields of light} → {changed}"),
        CommandSpec::new("lightlist", "Light List", run_list)
            .menu(&["View", "Render", "Light List"])
            .params("{} → {lights: [{handle, name, kind, on, position, target, intensity, color}]}")
            .noundo(),
        CommandSpec::new("sunproperties", "Sun Properties", run_sun)
            .menu(&["View", "Render", "Sun Properties"])
            .params("{on?, date?: \"YYYY-MM-DD\", time?: \"HH:MM\", latitude?, longitude? (degrees, east +), timezone? (hours from UTC), north? (degrees counterclockwise from +Y), intensity?} → the sun, with its elevation and azimuth"),
        CommandSpec::new("renderexposure", "Render Exposure", run_exposure)
            .menu(&["View", "Render", "Render Exposure"])
            .params("{exposure?: stops (−20 … 20; + brighter)} → {exposure}"),
    ]
}

fn num(p: &Value, key: &str) -> Result<Option<f64>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_f64().filter(|x| x.is_finite()).map(Some).ok_or_else(|| bad("light", format!("`{key}` must be a number"))),
    }
}

fn point3(p: &Value, key: &str) -> Result<Option<Vec3>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(a)) if (2..=3).contains(&a.len()) => {
            let c: Option<Vec<f64>> = a.iter().map(|v| v.as_f64().filter(|x| x.is_finite())).collect();
            let c = c.ok_or_else(|| bad("light", format!("`{key}` must be [x, y, z]")))?;
            Ok(Some(Vec3::new(c.first().copied().unwrap_or(0.0), c.get(1).copied().unwrap_or(0.0), c.get(2).copied().unwrap_or(0.0))))
        }
        Some(_) => Err(bad("light", format!("`{key}` must be [x, y, z]"))),
    }
}

/// Apply the light fields of `p` to `l`.
fn apply(l: &mut Light, p: &Value) -> Result<()> {
    if let Some(k) = str_param(p, "kind") {
        l.kind = LightKind::parse(k).ok_or_else(|| bad("light", format!("unknown light kind `{k}` (point, spot or distant)")))?;
    }
    if let Some(v) = point3(p, "position")? {
        l.position = v;
    }
    if let Some(v) = point3(p, "target")? {
        l.target = v;
    }
    if let Some(v) = num(p, "intensity")? {
        l.intensity = v;
    }
    if let Some(v) = num(p, "hotspot")? {
        l.hotspot = v;
    }
    if let Some(v) = num(p, "falloff")? {
        l.falloff = v;
    }
    if let Some(n) = str_param(p, "name") {
        l.name = n.to_string();
    }
    for (key, field) in [("on", &mut l.on), ("shadows", &mut l.shadows)] {
        if let Some(b) = p.get(key).and_then(Value::as_bool) {
            *field = b;
        }
    }
    if let Some(a) = str_param(p, "attenuation") {
        l.attenuation = match a.trim().to_ascii_lowercase().as_str() {
            "none" => Attenuation::None,
            "linear" | "inverselinear" => Attenuation::InverseLinear,
            "square" | "inversesquare" => Attenuation::InverseSquare,
            _ => return Err(bad("light", format!("unknown attenuation `{a}` (none, linear or square)"))),
        };
    }
    Ok(())
}

fn color(p: &Value) -> Result<Option<cadaddy_doc::color::Color>> {
    match p.get("color") {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_i64()
            .filter(|c| (1..=255).contains(c))
            .map(|c| Some(cadaddy_doc::color::Color::Index(c as u8)))
            .ok_or_else(|| bad("light", "`color` must be a colour index 1–255")),
    }
}

fn make(s: &mut Session, p: &Value, kind: Option<LightKind>) -> Result<Value> {
    let mut l = Light { kind: kind.unwrap_or_default(), ..Light::default() };
    apply(&mut l, p)?;
    if point3(p, "position")?.is_none() {
        return Err(bad("light", "`position` is required"));
    }
    if point3(p, "target")?.is_none() {
        l.target = l.position - Vec3::new(0.0, 0.0, 1.0);
    }
    if l.name.is_empty() {
        let n = s.doc()?.model.iter().filter(|e| matches!(e.kind, EntityKind::Light(_))).count() + 1;
        l.name = format!(
            "{}light{n}",
            match l.kind {
                LightKind::Point => "Point",
                LightKind::Spot => "Spot",
                LightKind::Distant => "Distant",
            }
        );
    }
    let l = l.sane();
    let mut common = s.current_common();
    if let Some(c) = color(p)? {
        common.color = c;
    }
    let space = s.space();
    let h = s.doc_mut()?.add(&space, common, EntityKind::Light(l))?;
    Ok(json!({ "handle": h.hex() }))
}

fn run_light(s: &mut Session, p: &Value) -> Result<Value> {
    make(s, p, None)
}

fn run_kind(s: &mut Session, p: &Value, kind: LightKind) -> Result<Value> {
    if str_param(p, "kind").is_some() {
        return Err(bad("light", "`kind` is the command's own"));
    }
    make(s, p, Some(kind))
}

fn run_set(s: &mut Session, p: &Value) -> Result<Value> {
    let hs = targets(s, p)?;
    // Check every field on a scratch light before changing any.
    apply(&mut Light::default(), p)?;
    let c = color(p)?;
    let d = s.doc_mut()?;
    let mut n = 0usize;
    for h in hs {
        if !d.entity(h).is_some_and(|e| matches!(e.kind, EntityKind::Light(_))) {
            continue;
        }
        let ok = d
            .modify_entity(h, |e| {
                if let EntityKind::Light(l) = &mut e.kind {
                    let _ = apply(l, p);
                    *l = l.clone().sane();
                }
                if let Some(c) = c {
                    e.common.color = c;
                }
            })
            .is_ok();
        n += usize::from(ok);
    }
    Ok(json!({ "changed": n }))
}

fn run_list(s: &mut Session, _p: &Value) -> Result<Value> {
    let d = s.doc()?;
    let lights: Vec<Value> = d
        .model
        .iter()
        .filter_map(|e| {
            let EntityKind::Light(l) = &e.kind else { return None };
            Some(json!({
                "handle": e.handle.hex(), "name": l.name, "kind": l.kind.as_str(), "on": l.on,
                "position": [l.position.x, l.position.y, l.position.z], "target": [l.target.x, l.target.y, l.target.z],
                "intensity": l.intensity, "color": serde_json::to_value(e.common.color).unwrap_or(Value::Null),
            }))
        })
        .collect();
    Ok(json!({ "lights": lights }))
}

fn sun_json(sun: &Sun) -> Value {
    let (el, az) = sun.elevation_azimuth();
    json!({
        "on": sun.on, "date": format!("{:04}-{:02}-{:02}", sun.year, sun.month, sun.day), "time": format!("{:02}:{:02}", sun.minutes / 60, sun.minutes % 60),
        "latitude": sun.latitude, "longitude": sun.longitude, "timezone": sun.timezone, "north": sun.north, "intensity": sun.intensity,
        "exposure": sun.exposure, "elevation": el, "azimuth": az,
    })
}

fn run_sun(s: &mut Session, p: &Value) -> Result<Value> {
    let mut sun = s.doc()?.sun.clone();
    if let Some(b) = p.get("on").and_then(Value::as_bool) {
        sun.on = b;
    }
    if let Some(date) = str_param(p, "date") {
        let parts: Vec<&str> = date.trim().split('-').collect();
        let [y, m, d] = parts.as_slice() else { return Err(bad("sunproperties", "`date` must be YYYY-MM-DD")) };
        let parse = |t: &str| t.trim().parse::<i64>().ok();
        match (parse(y), parse(m), parse(d)) {
            (Some(y), Some(m), Some(d)) if (1..=9999).contains(&y) && (1..=12).contains(&m) && (1..=31).contains(&d) => {
                (sun.year, sun.month, sun.day) = (y as i32, m as u32, d as u32);
            }
            _ => return Err(bad("sunproperties", "`date` must be YYYY-MM-DD")),
        }
    }
    if let Some(time) = str_param(p, "time") {
        let (h, m) = time.trim().split_once(':').ok_or_else(|| bad("sunproperties", "`time` must be HH:MM"))?;
        match (h.trim().parse::<u32>(), m.trim().parse::<u32>()) {
            (Ok(h), Ok(m)) if h < 24 && m < 60 => sun.minutes = h * 60 + m,
            _ => return Err(bad("sunproperties", "`time` must be HH:MM")),
        }
    }
    for (key, field, lo, hi) in [
        ("latitude", &mut sun.latitude, -90.0, 90.0),
        ("longitude", &mut sun.longitude, -180.0, 180.0),
        ("timezone", &mut sun.timezone, -14.0, 14.0),
        ("north", &mut sun.north, -360.0, 360.0),
        ("intensity", &mut sun.intensity, 0.0, 100.0),
    ] {
        if let Some(v) = num(p, key)? {
            if !(lo..=hi).contains(&v) {
                return Err(bad("sunproperties", format!("`{key}` must be from {lo} to {hi}")));
            }
            *field = v;
        }
    }
    let sun = sun.sane();
    let r = sun_json(&sun);
    s.doc_mut()?.sun = sun;
    Ok(r)
}

fn run_exposure(s: &mut Session, p: &Value) -> Result<Value> {
    if let Some(v) = num(p, "exposure")? {
        if !(-20.0..=20.0).contains(&v) {
            return Err(bad("renderexposure", "`exposure` must be from -20 to 20 stops"));
        }
        s.doc_mut()?.sun.exposure = v;
    }
    Ok(json!({ "exposure": s.doc()?.sun.exposure }))
}

/// LIGHT at the command line: the kind (unless the command names it), the position, then where
/// a spot or distant light points.
struct LightM {
    kind: Option<LightKind>,
    position: Option<Vec3>,
}

impl LightM {
    fn new(kind: Option<LightKind>) -> Self {
        LightM { kind, position: None }
    }
    fn place(&mut self, s: &mut Session, target: Option<Vec3>) -> Result<Step> {
        let Some(pos) = self.position else { return Ok(Step::Continue) };
        let mut p = json!({ "kind": self.kind.unwrap_or_default().as_str(), "position": [pos.x, pos.y, pos.z] });
        if let Some(t) = target {
            p["target"] = json!([t.x, t.y, t.z]);
        }
        match make(s, &p, None) {
            Ok(_) => Ok(Step::Done),
            Err(e) => {
                s.echo(e.to_string());
                Ok(Step::Done)
            }
        }
    }
}

impl Interactive for LightM {
    fn name(&self) -> &'static str {
        "LIGHT"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        match (self.kind, self.position) {
            (None, _) => Prompt::new("Enter light type", Accept::TEXT).kw(&["Point", "Spot", "Distant"]).default("Point"),
            (Some(LightKind::Distant), None) => Prompt::new("Specify light direction FROM", Accept::POINT),
            (Some(_), None) => Prompt::new("Specify source location", Accept::POINT),
            (Some(LightKind::Distant), Some(_)) => Prompt::new("Specify light direction TO", Accept::POINT),
            (Some(_), Some(_)) => Prompt::new("Specify target location", Accept::POINT),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if matches!(i, Input::Cancel) {
            return Ok(Step::Cancel);
        }
        if self.kind.is_none() {
            let t = match &i {
                Input::Text(t) | Input::Keyword(t) => t.trim().to_string(),
                _ => "point".into(),
            };
            match LightKind::parse(if t.is_empty() { "point" } else { &t }) {
                Some(k) => self.kind = Some(k),
                None => s.echo("Invalid option keyword."),
            }
            return Ok(Step::Continue);
        }
        let Input::Point(pt) = i else {
            return Ok(Step::Continue);
        };
        let p3 = |q: Vec2| Vec3::new(q.x, q.y, 0.0);
        match self.position {
            None => {
                self.position = Some(p3(pt));
                if self.kind == Some(LightKind::Point) {
                    return self.place(s, None);
                }
                Ok(Step::Continue)
            }
            Some(_) => self.place(s, Some(p3(pt))),
        }
    }
}

/// The lights of `hs` (light objects only), for tests and the properties palette.
pub fn lights_of(s: &Session, hs: &[Handle]) -> Vec<Light> {
    let Ok(d) = s.doc() else { return Vec::new() };
    hs.iter().filter_map(|h| if let EntityKind::Light(l) = &d.entity(*h)?.kind { Some(l.clone()) } else { None }).collect()
}
