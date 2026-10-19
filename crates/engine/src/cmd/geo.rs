//! GEOGRAPHICLOCATION, GEOREMOVE and LATLONG: where the drawing is on the earth, without online
//! maps (no map tiles). The location is a latitude, longitude and elevation at a design point in
//! the drawing, the direction of true north, the size of a drawing unit, and a coordinate system
//! (latitude/longitude or a UTM zone). Setting it also places the sun for rendered views and sun
//! studies. LATLONG converts between drawing points and latitude/longitude (or UTM grid
//! coordinates); at any point prompt `LATLONG` takes a typed latitude and longitude.

use cadaddy_doc::geo::{CoordSystem, GeoLocation, utm_forward, utm_inverse};
use cadaddy_geom::{Vec2, Vec3};
use serde_json::{Value, json};

use super::*;
use crate::{Accept, Input, Interactive, Prompt, Result, Session, Step};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("geographiclocation", "Set Location", run_set)
            .menu(&["Tools", "Geographic Location..."])
            .alias(&["geo"])
            .params(
                "{latitude, longitude (degrees, north and east +), elevation? (m), point?: [x,y] (the design point, default 0,0), north?: degrees counterclockwise from +Y | northDirection?: [x,y], unit?: metres per drawing unit (default from INSUNITS), system?: \"LL84\" | \"UTM84-14N\" | \"UTM\" (the zone holding the location), sun?: bool (default true: the sun takes the location and north)} → the location | {} → the location (or null)",
            )
            .interactive(|_| Ok(Box::new(GeoM::default()))),
        CommandSpec::new("georemove", "Remove Location", run_remove).menu(&["Tools", "Remove Geographic Location"]),
        CommandSpec::new("latlong", "Latitude and Longitude", run_latlong)
            .params("{point: [x,y]} → {latitude, longitude, easting?, northing? (UTM systems)} | {latitude, longitude} → {point} | {easting, northing} (UTM) → {point, latitude, longitude}")
            .noundo(),
    ]
}

fn geo_json(g: &GeoLocation) -> Value {
    let mut v = json!({
        "latitude": g.latitude, "longitude": g.longitude, "elevation": g.elevation,
        "point": [g.design_point.x, g.design_point.y], "north": g.north, "unit": g.unit_m, "system": g.system.code(),
    });
    if let CoordSystem::Utm { zone, north } = g.system {
        let (e, n) = utm_forward(g.latitude, g.longitude, zone, north);
        v["easting"] = json!(e);
        v["northing"] = json!(n);
    }
    v
}

fn num(cmd: &str, p: &Value, key: &str, lo: f64, hi: f64) -> Result<Option<f64>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_f64()
            .filter(|x| x.is_finite() && (lo..=hi).contains(x))
            .map(Some)
            .ok_or_else(|| bad(cmd, format!("`{key}` must be a number from {lo} to {hi}"))),
    }
}

fn run_set(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "geographiclocation";
    let has = |k: &str| p.get(k).is_some_and(|v| !v.is_null());
    if !["latitude", "longitude", "point", "north", "northDirection", "unit", "system", "elevation"].iter().any(|k| has(k)) {
        return Ok(s.doc()?.geo.as_ref().map_or(Value::Null, geo_json));
    }
    let d = s.doc()?;
    let mut g = d
        .geo
        .clone()
        .unwrap_or_else(|| GeoLocation { unit_m: cadaddy_doc::geo::metres_per_unit(d.header.i64("INSUNITS", 0)), ..GeoLocation::default() });
    let fresh = d.geo.is_none();
    if fresh && !(has("latitude") && has("longitude")) {
        return Err(bad(CMD, "`latitude` and `longitude` are required"));
    }
    if let Some(v) = num(CMD, p, "latitude", -90.0, 90.0)? {
        g.latitude = v;
    }
    if let Some(v) = num(CMD, p, "longitude", -180.0, 180.0)? {
        g.longitude = v;
    }
    if let Some(v) = num(CMD, p, "elevation", -20_000.0, 100_000.0)? {
        g.elevation = v;
    }
    if let Some(v) = p.get("point").filter(|v| !v.is_null()) {
        let pt = point_value(v).ok_or_else(|| bad(CMD, "`point` must be [x, y]"))?;
        g.design_point = Vec3::new(pt.x, pt.y, 0.0);
    }
    if let Some(v) = num(CMD, p, "north", -360.0, 360.0)? {
        g.north = v.rem_euclid(360.0);
    }
    if let Some(v) = p.get("northDirection").filter(|v| !v.is_null()) {
        let dir = point_value(v).filter(|d| d.len() > 1e-12).ok_or_else(|| bad(CMD, "`northDirection` must be a non-zero [x, y]"))?;
        g.north = (-dir.x).atan2(dir.y).to_degrees().rem_euclid(360.0);
    }
    if let Some(v) = num(CMD, p, "unit", 1e-12, 1e12)? {
        g.unit_m = v;
    }
    if let Some(t) = str_param(p, "system") {
        g.system = if t.trim().eq_ignore_ascii_case("utm") {
            CoordSystem::utm_for(g.latitude, g.longitude)
        } else {
            CoordSystem::parse(t).ok_or_else(|| bad(CMD, format!("unknown coordinate system `{t}` (LL84, UTM84-<zone><N|S>, UTM)")))?
        };
    }
    if !g.is_valid() {
        return Err(bad(CMD, "the location is not valid"));
    }
    let sun = bool_or(p, "sun", true);
    let d = s.doc_mut()?;
    if sun {
        d.sun.latitude = g.latitude;
        d.sun.longitude = g.longitude;
        d.sun.north = g.north;
    }
    d.geo = Some(g.clone());
    let mut out = geo_json(&g);
    out["message"] = json!(format!("Location set: {:.6}, {:.6} ({}).", g.latitude, g.longitude, g.system.code()));
    Ok(out)
}

fn run_remove(s: &mut Session, _p: &Value) -> Result<Value> {
    if s.doc()?.geo.is_none() {
        return Ok(json!({ "removed": false, "message": "The drawing has no geographic location." }));
    }
    s.doc_mut()?.geo = None;
    Ok(json!({ "removed": true, "message": "Geographic location removed." }))
}

fn run_latlong(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "latlong";
    let g = s.doc()?.geo.clone().ok_or_else(|| bad(CMD, "the drawing has no geographic location (GEOGRAPHICLOCATION)"))?;
    if let Some(v) = p.get("point").filter(|v| !v.is_null()) {
        let pt = point_value(v).ok_or_else(|| bad(CMD, "`point` must be [x, y]"))?;
        let (lat, lon) = g.to_latlong(pt);
        let mut out = json!({ "latitude": lat, "longitude": lon });
        if let CoordSystem::Utm { zone, north } = g.system {
            let (e, n) = utm_forward(lat, lon, zone, north);
            out["easting"] = json!(e);
            out["northing"] = json!(n);
        }
        return Ok(out);
    }
    let (lat, lon) = match (num(CMD, p, "latitude", -90.0, 90.0)?, num(CMD, p, "longitude", -180.0, 180.0)?) {
        (Some(a), Some(b)) => (a, b),
        _ => {
            let (Some(e), Some(n)) = (num(CMD, p, "easting", -1e8, 1e8)?, num(CMD, p, "northing", -1e8, 1e8)?) else {
                return Err(bad(CMD, "pass `point`, or `latitude` and `longitude`, or `easting` and `northing`"));
            };
            let CoordSystem::Utm { zone, north } = g.system else {
                return Err(bad(CMD, "`easting`/`northing` need a UTM coordinate system"));
            };
            utm_inverse(e, n, zone, north)
        }
    };
    let pt = g.from_latlong(lat, lon);
    Ok(json!({ "point": [pt.x, pt.y], "latitude": lat, "longitude": lon }))
}

/// GEOGRAPHICLOCATION typed: latitude, longitude, the design point and the north direction.
#[derive(Default)]
struct GeoM {
    lat: Option<f64>,
    lon: Option<f64>,
    point: Option<Vec2>,
}

impl Interactive for GeoM {
    fn name(&self) -> &'static str {
        "GEOGRAPHICLOCATION"
    }
    fn prompt(&self, s: &Session) -> Prompt {
        let cur = s.doc().ok().and_then(|d| d.geo.clone());
        match (self.lat, self.lon, self.point) {
            (None, _, _) => {
                let p = Prompt::new("Enter latitude (degrees, north +)", Accept::TEXT);
                match cur.as_ref() {
                    Some(g) => p.kw(&["Remove"]).default(format!("{}", g.latitude)),
                    None => p,
                }
            }
            (Some(_), None, _) => {
                let p = Prompt::new("Enter longitude (degrees, east +)", Accept::TEXT);
                match cur.as_ref() {
                    Some(g) => p.default(format!("{}", g.longitude)),
                    None => p,
                }
            }
            (Some(_), Some(_), None) => Prompt::new("Specify design point", Accept::POINT).default("0,0"),
            _ => Prompt::new("Specify north direction angle (degrees from +Y, counterclockwise)", Accept::TEXT)
                .default("0")
                .base(self.point.unwrap_or(Vec2::ZERO)),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        let cur = s.doc()?.geo.clone();
        let text = match &i {
            Input::Text(t) => Some(t.trim().to_string()),
            Input::Enter => Some(String::new()),
            _ => None,
        };
        match (self.lat, self.lon, self.point) {
            (None, _, _) => {
                if let Input::Keyword(k) = &i
                    && k == "Remove"
                {
                    let r = run_remove(s, &json!({}))?;
                    s.echo(r["message"].as_str().unwrap_or_default().to_string());
                    return Ok(Step::Done);
                }
                let t = text.unwrap_or_default();
                let v = if t.is_empty() { cur.map(|g| g.latitude) } else { t.parse::<f64>().ok() };
                match v.filter(|v| v.is_finite() && (-90.0..=90.0).contains(v)) {
                    Some(v) => self.lat = Some(v),
                    None => s.echo("Latitude is a number from -90 to 90."),
                }
            }
            (Some(_), None, _) => {
                let t = text.unwrap_or_default();
                let v = if t.is_empty() { cur.map(|g| g.longitude) } else { t.parse::<f64>().ok() };
                match v.filter(|v| v.is_finite() && (-180.0..=180.0).contains(v)) {
                    Some(v) => self.lon = Some(v),
                    None => s.echo("Longitude is a number from -180 to 180."),
                }
            }
            (Some(_), Some(_), None) => match i {
                Input::Point(p) => self.point = Some(p),
                Input::Enter => self.point = Some(Vec2::ZERO),
                _ => {}
            },
            (Some(lat), Some(lon), Some(p)) => {
                let north = match i {
                    // A point picked: north points from the design point towards it.
                    Input::Point(q) if (q - p).len() > 1e-12 => (-(q - p).x).atan2((q - p).y).to_degrees(),
                    _ => match text.as_deref() {
                        Some("") => 0.0,
                        Some(t) => match t.parse::<f64>() {
                            Ok(v) if v.is_finite() => v,
                            _ => {
                                s.echo("Enter an angle in degrees.");
                                return Ok(Step::Continue);
                            }
                        },
                        None => return Ok(Step::Continue),
                    },
                };
                let r = run_set(s, &json!({ "latitude": lat, "longitude": lon, "point": [p.x, p.y], "north": north }))?;
                s.echo(r["message"].as_str().unwrap_or_default().to_string());
                return Ok(Step::Done);
            }
        }
        Ok(Step::Continue)
    }
}
