//! Camera tours: named viewpoints (a place, a heading, a level, the room it shows) and paths
//! through them — a smooth curve through the viewpoints' eyes, walked at a speed, looking along
//! the path, turning between the viewpoints' own headings, or at a fixed point. Kept with the
//! drawing as JSON ([`Tours`]); played in Walk and exported frame by frame.
//!
//! Units are walk units: feet, x east, y north, z up; headings in degrees from east toward north.

use crate::math::{V3, v3};
use serde::{Deserialize, Serialize};

/// The version written into the JSON.
pub const TOURS_VERSION: u32 = 1;
/// Most viewpoints and tours kept (hostile files).
pub const MAX_VIEWPOINTS: usize = 2_000;
pub const MAX_TOURS: usize = 500;
/// Most viewpoints one tour passes through.
pub const MAX_TOUR_STOPS: usize = 500;
/// Most frames one export makes (an hour at 30 fps).
pub const MAX_FRAMES: usize = 108_000;
/// Speeds a tour may have, feet per second.
pub const MIN_SPEED: f64 = 0.05;
pub const MAX_SPEED: f64 = 500.0;
/// The pace a tour walks at unless it says otherwise, feet per second.
pub const DEFAULT_SPEED: f64 = 4.5;
/// Curve samples per segment for arc length.
const SAMPLES: usize = 48;
/// Coordinates further out than this are refused, feet.
const FAR: f64 = 1e7;

/// A saved viewpoint.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Viewpoint {
    pub name: String,
    /// The room it shows ("" for none).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub room: String,
    #[serde(default)]
    pub level: usize,
    /// The eye, feet.
    pub eye: [f64; 3],
    /// Heading, degrees from east toward north.
    #[serde(default)]
    pub yaw: f64,
    /// Up (+) or down (−), degrees.
    #[serde(default)]
    pub pitch: f64,
}

/// Where a tour's camera looks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Look {
    /// Along the path (ahead of the camera).
    #[default]
    Path,
    /// Each viewpoint's own heading, turning smoothly between them.
    Views,
    /// Always at this point, feet.
    At([f64; 3]),
}

/// A camera path through viewpoints.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tour {
    pub name: String,
    /// The viewpoints passed through, by name, in order.
    pub stops: Vec<String>,
    /// Feet per second along the path.
    #[serde(default = "default_speed")]
    pub speed: f64,
    #[serde(default)]
    pub look: Look,
    /// Seconds held still at each stop (the first and last included).
    #[serde(default)]
    pub pause: f64,
}

fn default_speed() -> f64 {
    DEFAULT_SPEED
}

/// Everything kept with a drawing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Tours {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub views: Vec<Viewpoint>,
    #[serde(default)]
    pub tours: Vec<Tour>,
}

/// A camera pose on a tour.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub eye: V3,
    /// Radians, as [`crate::Walker`] has them.
    pub yaw: f64,
    pub pitch: f64,
    pub level: usize,
}

fn finite3(p: [f64; 3]) -> bool {
    p.iter().all(|c| c.is_finite() && c.abs() < FAR)
}

impl Tours {
    /// Read the drawing's tours; `None` text reads as none saved.
    pub fn from_json(text: Option<&str>) -> Result<Tours, String> {
        let Some(text) = text.filter(|t| !t.trim().is_empty()) else { return Ok(Tours { version: TOURS_VERSION, ..Tours::default() }) };
        let mut t: Tours = serde_json::from_str(text).map_err(|e| format!("tours: {e}"))?;
        if t.version > TOURS_VERSION {
            return Err(format!("tours: version {} is newer than this CADaddy reads ({TOURS_VERSION})", t.version));
        }
        t.version = TOURS_VERSION;
        t.views.truncate(MAX_VIEWPOINTS);
        t.tours.truncate(MAX_TOURS);
        t.views.retain(|v| finite3(v.eye) && v.yaw.is_finite() && v.pitch.is_finite());
        for tour in &mut t.tours {
            tour.stops.truncate(MAX_TOUR_STOPS);
            if !tour.speed.is_finite() {
                tour.speed = DEFAULT_SPEED;
            }
            tour.speed = tour.speed.clamp(MIN_SPEED, MAX_SPEED);
            tour.pause = if tour.pause.is_finite() { tour.pause.clamp(0.0, 600.0) } else { 0.0 };
            if let Look::At(p) = tour.look
                && !finite3(p)
            {
                tour.look = Look::Path;
            }
        }
        Ok(t)
    }

    /// The JSON kept with the drawing (`None` when nothing is saved).
    pub fn to_json(&self) -> Option<String> {
        if self.views.is_empty() && self.tours.is_empty() {
            return None;
        }
        serde_json::to_string(&Tours { version: TOURS_VERSION, ..self.clone() }).ok()
    }

    pub fn view(&self, name: &str) -> Option<&Viewpoint> {
        self.views.iter().find(|v| v.name.eq_ignore_ascii_case(name))
    }

    pub fn tour(&self, name: &str) -> Option<&Tour> {
        self.tours.iter().find(|t| t.name.eq_ignore_ascii_case(name))
    }

    /// Add a viewpoint, replacing one of the same name.
    pub fn put_view(&mut self, v: Viewpoint) -> Result<(), String> {
        if !finite3(v.eye) || !v.yaw.is_finite() || !v.pitch.is_finite() {
            return Err("the viewpoint's eye and heading must be finite numbers".into());
        }
        let full = self.views.len() >= MAX_VIEWPOINTS;
        match self.views.iter_mut().find(|o| o.name.eq_ignore_ascii_case(&v.name)) {
            Some(o) => *o = v,
            None if full => return Err(format!("at most {MAX_VIEWPOINTS} viewpoints")),
            None => self.views.push(v),
        }
        Ok(())
    }

    /// Add a tour, replacing one of the same name; every stop must be a saved viewpoint.
    pub fn put_tour(&mut self, t: Tour) -> Result<(), String> {
        if let Some(missing) = t.stops.iter().find(|s| self.view(s).is_none()) {
            return Err(format!("no viewpoint named {missing}"));
        }
        self.path(&t)?;
        let full = self.tours.len() >= MAX_TOURS;
        match self.tours.iter_mut().find(|o| o.name.eq_ignore_ascii_case(&t.name)) {
            Some(o) => *o = t,
            None if full => return Err(format!("at most {MAX_TOURS} tours")),
            None => self.tours.push(t),
        }
        Ok(())
    }

    /// The path a tour follows.
    pub fn path(&self, t: &Tour) -> Result<TourPath, String> {
        if t.stops.is_empty() {
            return Err(format!("tour {} has no viewpoints", t.name));
        }
        if t.stops.len() > MAX_TOUR_STOPS {
            return Err(format!("a tour passes at most {MAX_TOUR_STOPS} viewpoints"));
        }
        let keys = t
            .stops
            .iter()
            .map(|s| self.view(s).cloned().ok_or_else(|| format!("tour {}: no viewpoint named {s}", t.name)))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(TourPath::new(keys, t.speed, t.look.clone(), t.pause))
    }
}

/// A tour's curve, measured: a centripetal Catmull–Rom spline through the stops' eyes.
#[derive(Clone, Debug)]
pub struct TourPath {
    keys: Vec<Viewpoint>,
    speed: f64,
    look: Look,
    pause: f64,
    /// Per segment, the arc length at each of its samples (from the path start).
    table: Vec<Vec<f64>>,
}

impl TourPath {
    fn new(keys: Vec<Viewpoint>, speed: f64, look: Look, pause: f64) -> TourPath {
        let speed = if speed.is_finite() { speed.clamp(MIN_SPEED, MAX_SPEED) } else { DEFAULT_SPEED };
        let pause = if pause.is_finite() { pause.clamp(0.0, 600.0) } else { 0.0 };
        let mut p = TourPath { keys, speed, look, pause, table: Vec::new() };
        let mut s = 0.0;
        for seg in 0..p.keys.len().saturating_sub(1) {
            let mut row = Vec::with_capacity(SAMPLES + 1);
            let mut prev = p.point(seg, 0.0);
            row.push(s);
            for i in 1..=SAMPLES {
                let q = p.point(seg, i as f64 / SAMPLES as f64);
                s += (q - prev).len();
                row.push(s);
                prev = q;
            }
            p.table.push(row);
        }
        p
    }

    fn eye(&self, i: usize) -> V3 {
        let n = self.keys.len();
        let k = self.keys.get(i.min(n.saturating_sub(1)));
        k.map_or(V3::default(), |k| v3(k.eye[0], k.eye[1], k.eye[2]))
    }

    /// The point `u` (0..1) along segment `seg` (from stop `seg` to stop `seg + 1`).
    fn point(&self, seg: usize, u: f64) -> V3 {
        let p1 = self.eye(seg);
        let p2 = self.eye(seg + 1);
        // The ends are mirrored so the curve starts and ends heading at its neighbour.
        let p0 = if seg == 0 { p1 * 2.0 - p2 } else { self.eye(seg - 1) };
        let p3 = if seg + 2 >= self.keys.len() { p2 * 2.0 - p1 } else { self.eye(seg + 2) };
        catmull_rom(p0, p1, p2, p3, u)
    }

    /// Path length, feet.
    pub fn length(&self) -> f64 {
        self.table.last().and_then(|r| r.last()).copied().unwrap_or(0.0)
    }

    /// Seconds from start to finish (pauses included).
    pub fn duration(&self) -> f64 {
        self.length() / self.speed + self.pause * self.keys.len() as f64
    }

    /// Frames an export at `fps` makes: one at the start, then one every 1/fps s up to the end.
    pub fn frame_count(&self, fps: f64) -> usize {
        if !fps.is_finite() || fps <= 0.0 {
            return 0;
        }
        let n = (self.duration() * fps + 1e-9).floor();
        if n.is_finite() && n >= 0.0 { (n as usize).saturating_add(1).min(MAX_FRAMES) } else { 1 }
    }

    pub fn stops(&self) -> &[Viewpoint] {
        &self.keys
    }

    /// Where along the path (arc length) the camera is at `t` seconds, and the stop it's held at.
    fn distance_at(&self, t: f64) -> (f64, Option<usize>) {
        let t = if t.is_finite() { t.max(0.0) } else { 0.0 };
        let mut clock = 0.0;
        for i in 0..self.keys.len() {
            let at = self.table.get(i).and_then(|r| r.first()).copied().unwrap_or_else(|| self.length());
            if t < clock + self.pause {
                return (at, Some(i));
            }
            clock += self.pause;
            let Some(row) = self.table.get(i) else { break };
            let seg_len = row.last().copied().unwrap_or(at) - at;
            let seg_t = seg_len / self.speed;
            if t < clock + seg_t {
                return (at + (t - clock) * self.speed, None);
            }
            clock += seg_t;
        }
        (self.length(), self.keys.len().checked_sub(1))
    }

    /// The segment and its parameter at arc length `s`.
    fn locate(&self, s: f64) -> (usize, f64) {
        for (seg, row) in self.table.iter().enumerate() {
            let end = row.last().copied().unwrap_or(0.0);
            if s <= end || seg + 1 == self.table.len() {
                // Samples are increasing; find the pair around s.
                let i = row.partition_point(|&x| x < s).clamp(1, row.len().saturating_sub(1).max(1));
                let (a, b) = (row.get(i - 1).copied().unwrap_or(0.0), row.get(i).copied().unwrap_or(0.0));
                let f = if b > a { ((s - a) / (b - a)).clamp(0.0, 1.0) } else { 0.0 };
                return (seg, ((i - 1) as f64 + f) / SAMPLES as f64);
            }
        }
        (0, 0.0)
    }

    /// The camera at `t` seconds (held at the end after it).
    pub fn pose(&self, t: f64) -> Pose {
        let (s, held) = self.distance_at(t);
        let Some(first) = self.keys.first() else { return Pose { eye: V3::default(), yaw: 0.0, pitch: 0.0, level: 0 } };
        if self.table.is_empty() {
            return self.pose_of(first, self.eye(0), v3(1.0, 0.0, 0.0), 0, 0.0);
        }
        let (seg, u) = match held {
            Some(i) if i >= self.table.len() => (self.table.len() - 1, 1.0),
            Some(i) => (i, 0.0),
            None => self.locate(s),
        };
        let eye = self.point(seg, u);
        let (a, b) = (self.point(seg, (u - 0.01).max(0.0)), self.point(seg, (u + 0.01).min(1.0)));
        let ahead = b - a;
        let key = self.keys.get(if u < 0.5 { seg } else { seg + 1 }).unwrap_or(first);
        self.pose_of(key, eye, ahead, seg, u)
    }

    fn pose_of(&self, key: &Viewpoint, eye: V3, ahead: V3, seg: usize, u: f64) -> Pose {
        let level = key.level;
        let (k0, k1) = (self.keys.get(seg), self.keys.get(seg + 1).or(self.keys.get(seg)));
        let lerp_pitch = || match (k0, k1) {
            (Some(a), Some(b)) => (a.pitch + (b.pitch - a.pitch) * smooth(u)).to_radians(),
            _ => key.pitch.to_radians(),
        };
        let (yaw, pitch) = match &self.look {
            Look::Path => {
                if ahead.x.hypot(ahead.y) > 1e-9 {
                    (ahead.y.atan2(ahead.x), lerp_pitch())
                } else {
                    (key.yaw.to_radians(), lerp_pitch())
                }
            }
            Look::Views => match (k0, k1) {
                (Some(a), Some(b)) => {
                    let d = (b.yaw - a.yaw + 540.0).rem_euclid(360.0) - 180.0;
                    ((a.yaw + d * smooth(u)).to_radians(), lerp_pitch())
                }
                _ => (key.yaw.to_radians(), key.pitch.to_radians()),
            },
            Look::At(p) => {
                let d = v3(p[0], p[1], p[2]) - eye;
                let flat = d.x.hypot(d.y);
                if flat > 1e-9 || d.z.abs() > 1e-9 { (d.y.atan2(d.x), d.z.atan2(flat)) } else { (key.yaw.to_radians(), 0.0) }
            }
        };
        Pose { eye, yaw: yaw.rem_euclid(std::f64::consts::TAU), pitch: pitch.clamp(-1.5, 1.5), level }
    }
}

/// Ease in and out (0..1 → 0..1).
fn smooth(u: f64) -> f64 {
    let u = u.clamp(0.0, 1.0);
    u * u * (3.0 - 2.0 * u)
}

/// Centripetal Catmull–Rom between p1 and p2 (no loops or cusps on uneven spacing).
fn catmull_rom(p0: V3, p1: V3, p2: V3, p3: V3, u: f64) -> V3 {
    let knot = |a: V3, b: V3| (b - a).len().sqrt().max(1e-6);
    let t0 = 0.0;
    let t1 = t0 + knot(p0, p1);
    let t2 = t1 + knot(p1, p2);
    let t3 = t2 + knot(p2, p3);
    let t = t1 + (t2 - t1) * u.clamp(0.0, 1.0);
    let mix = |a: V3, b: V3, ta: f64, tb: f64| if (tb - ta).abs() < 1e-12 { a } else { a * ((tb - t) / (tb - ta)) + b * ((t - ta) / (tb - ta)) };
    let a1 = mix(p0, p1, t0, t1);
    let a2 = mix(p1, p2, t1, t2);
    let a3 = mix(p2, p3, t2, t3);
    let b1 = mix(a1, a2, t0, t2);
    let b2 = mix(a2, a3, t1, t3);
    mix(b1, b2, t1, t2)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vp(name: &str, x: f64, y: f64, yaw: f64) -> Viewpoint {
        Viewpoint { name: name.into(), room: String::new(), level: 0, eye: [x, y, 5.5], yaw, pitch: 0.0 }
    }

    fn tours() -> Tours {
        let mut t = Tours::default();
        for v in [vp("A", 0.0, 0.0, 0.0), vp("B", 10.0, 0.0, 90.0), vp("C", 20.0, 0.0, 180.0)] {
            t.put_view(v).unwrap();
        }
        t
    }

    #[test]
    fn a_straight_path_is_timed_by_its_length_and_speed() {
        let mut t = tours();
        t.put_tour(Tour { name: "T".into(), stops: vec!["A".into(), "B".into(), "C".into()], speed: 5.0, look: Look::Path, pause: 0.0 }).unwrap();
        let p = t.path(t.tour("t").unwrap()).unwrap();
        assert!((p.length() - 20.0).abs() < 1e-6, "{}", p.length());
        assert!((p.duration() - 4.0).abs() < 1e-6);
        assert_eq!(p.frame_count(30.0), 121);
        assert_eq!(p.frame_count(0.0), 0);
        // Constant speed: at 1 s, 5 ft along.
        let q = p.pose(1.0);
        assert!((q.eye.x - 5.0).abs() < 0.05 && q.eye.y.abs() < 1e-9, "{q:?}");
        assert!(q.yaw.abs() < 1e-6 || (q.yaw - std::f64::consts::TAU).abs() < 1e-6);
        assert!((p.pose(99.0).eye.x - 20.0).abs() < 1e-9);
        assert!((p.pose(-1.0).eye.x).abs() < 1e-9);
    }

    #[test]
    fn pauses_hold_at_each_stop_and_views_turn_between_headings() {
        let mut t = tours();
        t.put_tour(Tour { name: "T".into(), stops: vec!["A".into(), "B".into()], speed: 10.0, look: Look::Views, pause: 2.0 }).unwrap();
        let p = t.path(t.tour("T").unwrap()).unwrap();
        assert!((p.duration() - 5.0).abs() < 1e-6, "1 s of path, 2 × 2 s held");
        assert!(p.pose(1.5).eye.x.abs() < 1e-9, "held at A");
        let mid = p.pose(2.5);
        assert!((mid.eye.x - 5.0).abs() < 0.05);
        assert!((mid.yaw.to_degrees() - 45.0).abs() < 1e-6, "{}", mid.yaw.to_degrees());
        assert!((p.pose(4.0).eye.x - 10.0).abs() < 1e-9 && (p.pose(4.0).yaw.to_degrees() - 90.0).abs() < 1e-6, "held at B");
    }

    #[test]
    fn a_curve_passes_through_every_stop_and_looks_at_a_point() {
        let mut t = Tours::default();
        for v in [vp("A", 0.0, 0.0, 0.0), vp("B", 10.0, 10.0, 0.0), vp("C", 20.0, 0.0, 0.0)] {
            t.put_view(v).unwrap();
        }
        let tour =
            Tour { name: "T".into(), stops: vec!["A".into(), "B".into(), "C".into()], speed: 1.0, look: Look::At([10.0, -10.0, 5.5]), pause: 0.0 };
        t.put_tour(tour.clone()).unwrap();
        let p = t.path(&tour).unwrap();
        assert!(p.length() > 28.28, "longer than the chords");
        let at_b = p.pose(p.length() / 2.0);
        assert!((at_b.eye.x - 10.0).abs() < 0.2 && (at_b.eye.y - 10.0).abs() < 0.2, "{at_b:?}");
        assert!((at_b.yaw.to_degrees() - 270.0).abs() < 2.0, "looks south at the point: {}", at_b.yaw.to_degrees());
    }

    #[test]
    fn bad_input_is_refused_or_cleaned() {
        let mut t = tours();
        assert!(t.put_view(Viewpoint { eye: [f64::NAN, 0.0, 0.0], ..vp("X", 0.0, 0.0, 0.0) }).is_err());
        assert!(t.put_tour(Tour { name: "T".into(), stops: vec!["A".into(), "Nope".into()], speed: 1.0, look: Look::Path, pause: 0.0 }).is_err());
        assert!(t.put_tour(Tour { name: "T".into(), stops: vec![], speed: 1.0, look: Look::Path, pause: 0.0 }).is_err());
        // One stop: a still frame.
        t.put_tour(Tour { name: "One".into(), stops: vec!["B".into()], speed: 1.0, look: Look::Path, pause: 0.0 }).unwrap();
        let p = t.path(t.tour("One").unwrap()).unwrap();
        assert_eq!((p.duration(), p.frame_count(24.0)), (0.0, 1));
        assert!((p.pose(3.0).eye.x - 10.0).abs() < 1e-9);
        // Round trip, and hostile JSON.
        let back = Tours::from_json(t.to_json().as_deref()).unwrap();
        assert_eq!(back, Tours { version: TOURS_VERSION, ..t.clone() });
        let hostile = r#"{"version":1,"views":[{"name":"A","eye":[1e300,0,0]}],"tours":[{"name":"T","stops":["A"],"speed":-5,"pause":1e9}]}"#;
        let h = Tours::from_json(Some(hostile)).unwrap();
        assert!(h.views.is_empty());
        assert_eq!((h.tours[0].speed, h.tours[0].pause), (MIN_SPEED, 600.0));
        assert!(h.path(&h.tours[0]).is_err(), "its stop is gone");
        assert!(Tours::from_json(Some(r#"{"version":99}"#)).is_err());
        assert_eq!(Tours::from_json(None).unwrap().to_json(), None);
    }
}
