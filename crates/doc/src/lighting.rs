//! Lighting for rendered views: LIGHT objects (point, spot and distant lights, an entity like any
//! other: its colour is the object's colour) and the drawing's sun (SUNPROPERTIES: date, time and
//! place, so the sun stands where it would) with the exposure rendered views use
//! (RENDEREXPOSURE).

use serde::{Deserialize, Serialize};

use crate::geom::Vec3;

/// What kind of light.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LightKind {
    /// Parallel rays along target − position (like the sun, anywhere).
    Distant,
    /// From a point, in every direction.
    #[default]
    Point,
    /// From a point towards its target, within a cone.
    Spot,
}

impl LightKind {
    /// The DXF code (group 70: 1 distant, 2 point, 3 spot).
    pub fn dxf(self) -> i64 {
        match self {
            LightKind::Distant => 1,
            LightKind::Point => 2,
            LightKind::Spot => 3,
        }
    }

    pub fn from_dxf(v: i64) -> LightKind {
        match v {
            1 => LightKind::Distant,
            3 => LightKind::Spot,
            _ => LightKind::Point,
        }
    }

    pub fn parse(s: &str) -> Option<LightKind> {
        match s.trim().to_ascii_lowercase().as_str() {
            "distant" | "d" => Some(LightKind::Distant),
            "point" | "p" => Some(LightKind::Point),
            "spot" | "s" => Some(LightKind::Spot),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            LightKind::Distant => "distant",
            LightKind::Point => "point",
            LightKind::Spot => "spot",
        }
    }
}

/// How a light fades with distance.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Attenuation {
    None,
    InverseLinear,
    /// Physically: 1 / d².
    #[default]
    InverseSquare,
}

impl Attenuation {
    pub fn dxf(self) -> i64 {
        match self {
            Attenuation::None => 0,
            Attenuation::InverseLinear => 1,
            Attenuation::InverseSquare => 2,
        }
    }

    pub fn from_dxf(v: i64) -> Attenuation {
        match v {
            0 => Attenuation::None,
            1 => Attenuation::InverseLinear,
            _ => Attenuation::InverseSquare,
        }
    }
}

/// A LIGHT object.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Light {
    pub name: String,
    pub kind: LightKind,
    pub on: bool,
    pub position: Vec3,
    /// Where a spot or distant light points.
    pub target: Vec3,
    /// Intensity (candela for point and spot lights at 1 m; a factor of the sun for distant).
    pub intensity: f64,
    pub attenuation: Attenuation,
    /// Spot cone: full brightness within the hotspot, none beyond the falloff (degrees).
    pub hotspot: f64,
    pub falloff: f64,
    pub shadows: bool,
}

impl Default for Light {
    fn default() -> Self {
        Light {
            name: String::new(),
            kind: LightKind::Point,
            on: true,
            position: Vec3::ZERO,
            target: Vec3::new(0.0, 0.0, -1.0),
            intensity: 1500.0,
            attenuation: Attenuation::InverseSquare,
            hotspot: 45.0,
            falloff: 50.0,
            shadows: true,
        }
    }
}

impl Light {
    /// The light with values in range (hostile input made safe).
    pub fn sane(mut self) -> Light {
        let ang = |v: f64, d: f64| if v.is_finite() { v.clamp(0.0, 160.0) } else { d };
        self.name = self.name.trim().chars().take(255).collect();
        self.intensity = if self.intensity.is_finite() { self.intensity.clamp(0.0, 1e9) } else { 1500.0 };
        self.hotspot = ang(self.hotspot, 45.0);
        self.falloff = ang(self.falloff, 50.0).max(self.hotspot);
        if !self.position.is_finite() {
            self.position = Vec3::ZERO;
        }
        if !self.target.is_finite() || (self.target - self.position).len() < 1e-12 {
            self.target = self.position + Vec3::new(0.0, 0.0, -1.0);
        }
        self
    }

    /// The unit direction the light points (spot and distant).
    pub fn direction(&self) -> Vec3 {
        let d = self.target - self.position;
        let l = d.len();
        if l > 1e-12 && l.is_finite() { d * (1.0 / l) } else { Vec3::new(0.0, 0.0, -1.0) }
    }
}

/// The drawing's sun and the exposure of rendered views.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Sun {
    pub on: bool,
    /// Date as year, month (1–12), day.
    pub year: i32,
    pub month: u32,
    pub day: u32,
    /// Local clock time, minutes after midnight.
    pub minutes: u32,
    /// Degrees north (+) / south (−), east (+) / west (−); hours from UTC.
    pub latitude: f64,
    pub longitude: f64,
    pub timezone: f64,
    /// Angle (degrees, counterclockwise from +Y) of true north in the drawing.
    pub north: f64,
    /// Brightness factor of the sun and sky.
    pub intensity: f64,
    /// Exposure of rendered views (RENDEREXPOSURE), in stops: + brighter, − darker.
    pub exposure: f64,
}

impl Default for Sun {
    fn default() -> Self {
        // An afternoon around the equinox at a mid latitude: a useful sun angle.
        Sun {
            on: true,
            year: 2026,
            month: 9,
            day: 21,
            minutes: 15 * 60,
            latitude: 40.0,
            longitude: -95.0,
            timezone: -6.0,
            north: 0.0,
            intensity: 1.0,
            exposure: 0.0,
        }
    }
}

impl Sun {
    pub fn sane(mut self) -> Sun {
        let fin = |v: f64, d: f64| if v.is_finite() { v } else { d };
        self.year = self.year.clamp(1, 9999);
        self.month = self.month.clamp(1, 12);
        self.day = self.day.clamp(1, 31);
        self.minutes = self.minutes.min(24 * 60 - 1);
        self.latitude = fin(self.latitude, 40.0).clamp(-90.0, 90.0);
        self.longitude = fin(self.longitude, 0.0).clamp(-180.0, 180.0);
        self.timezone = fin(self.timezone, 0.0).clamp(-14.0, 14.0);
        self.north = fin(self.north, 0.0).rem_euclid(360.0);
        self.intensity = fin(self.intensity, 1.0).clamp(0.0, 100.0);
        self.exposure = fin(self.exposure, 0.0).clamp(-20.0, 20.0);
        self
    }

    /// The sun's elevation above the horizon and azimuth (clockwise from north), degrees, by the
    /// NOAA solar position equations (fractional-year form).
    pub fn elevation_azimuth(&self) -> (f64, f64) {
        let s = self.clone().sane();
        let leap = (s.year % 4 == 0 && s.year % 100 != 0) || s.year % 400 == 0;
        let days_before = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
        let doy = days_before.get((s.month - 1) as usize).copied().unwrap_or(0) + s.day + u32::from(leap && s.month > 2);
        let hours = f64::from(s.minutes) / 60.0;
        let year_days = if leap { 366.0 } else { 365.0 };
        let g = 2.0 * std::f64::consts::PI / year_days * (f64::from(doy) - 1.0 + (hours - 12.0) / 24.0);
        let eqtime = 229.18 * (0.000075 + 0.001868 * g.cos() - 0.032077 * g.sin() - 0.014615 * (2.0 * g).cos() - 0.040849 * (2.0 * g).sin());
        let decl = 0.006918 - 0.399912 * g.cos() + 0.070257 * g.sin() - 0.006758 * (2.0 * g).cos() + 0.000907 * (2.0 * g).sin()
            - 0.002697 * (3.0 * g).cos()
            + 0.00148 * (3.0 * g).sin();
        let time_offset = eqtime + 4.0 * s.longitude - 60.0 * s.timezone;
        let tst = f64::from(s.minutes) + time_offset;
        let ha = (tst / 4.0 - 180.0).to_radians();
        let lat = s.latitude.to_radians();
        let cos_zen = (lat.sin() * decl.sin() + lat.cos() * decl.cos() * ha.cos()).clamp(-1.0, 1.0);
        let zen = cos_zen.acos();
        let elevation = 90.0 - zen.to_degrees();
        let sin_zen = zen.sin().max(1e-12);
        let cos_az = ((lat.sin() * cos_zen - decl.sin()) / (lat.cos() * sin_zen).max(1e-12)).clamp(-1.0, 1.0);
        let mut az = 180.0 - cos_az.acos().to_degrees();
        if ha > 0.0 {
            az = 360.0 - az;
        }
        (elevation, az.rem_euclid(360.0))
    }

    /// The unit direction towards the sun in drawing coordinates (+Z up, north at `north`
    /// degrees counterclockwise from +Y).
    pub fn direction(&self) -> Vec3 {
        let (el, az) = self.elevation_azimuth();
        let (el, az) = (el.to_radians(), az.to_radians());
        // Azimuth clockwise from north; north is +Y turned by `north` counterclockwise.
        let a = az - self.north.to_radians();
        Vec3::new(el.cos() * a.sin(), el.cos() * a.cos(), el.sin())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sun_stands_where_it_should() {
        // Clock noon at the equinox on the equator at its own meridian: nearly overhead (the
        // equation of time puts solar noon about 7 minutes later, ~1.9° short).
        let eq = Sun { year: 2026, month: 3, day: 20, minutes: 12 * 60, latitude: 0.0, longitude: 0.0, timezone: 0.0, ..Sun::default() };
        assert!(eq.elevation_azimuth().0 > 87.0, "{:?}", eq.elevation_azimuth());
        // Noon at 40° N near the September equinox: about 50° up, due south.
        let s = Sun { month: 9, day: 22, minutes: 12 * 60, latitude: 40.0, longitude: 0.0, timezone: 0.0, ..Sun::default() };
        let (el, az) = s.elevation_azimuth();
        assert!((el - 50.0).abs() < 1.5 && (az - 180.0).abs() < 3.0, "{el} {az}");
        // Midsummer noon at 40° N: about 73°; midwinter about 27°.
        let summer = Sun { month: 6, day: 21, ..s.clone() };
        let winter = Sun { month: 12, day: 21, ..s.clone() };
        assert!((summer.elevation_azimuth().0 - 73.4).abs() < 1.5);
        assert!((winter.elevation_azimuth().0 - 26.6).abs() < 1.5);
        // Morning: east; evening: west; night: below the horizon.
        let morning = Sun { minutes: 8 * 60, ..s.clone() };
        let evening = Sun { minutes: 16 * 60, ..s.clone() };
        let night = Sun { minutes: 0, ..s.clone() };
        assert!(morning.elevation_azimuth().1 < 180.0 && evening.elevation_azimuth().1 > 180.0);
        assert!(night.elevation_azimuth().0 < 0.0);
        // In the drawing: due south at noon is −Y (north +Y), and up.
        let d = s.direction();
        assert!(d.y < -0.5 && d.z > 0.7 && d.x.abs() < 0.1, "{d:?}");
        // North turned 90° counterclockwise: south is now +X.
        let turned = Sun { north: 90.0, ..s }.direction();
        assert!(turned.x > 0.5, "{turned:?}");
    }

    #[test]
    fn hostile_values_are_safe() {
        let s = Sun { month: 0, day: 99, minutes: 99_999, latitude: f64::NAN, exposure: 1e9, ..Sun::default() }.sane();
        assert_eq!((s.month, s.day, s.minutes, s.latitude, s.exposure), (1, 31, 1439, 40.0, 20.0));
        let (el, az) = Sun { latitude: f64::INFINITY, ..Sun::default() }.elevation_azimuth();
        assert!(el.is_finite() && az.is_finite());
        let l = Light { intensity: f64::NAN, hotspot: 300.0, falloff: 10.0, position: Vec3::new(f64::NAN, 0.0, 0.0), ..Light::default() }.sane();
        assert_eq!((l.intensity, l.hotspot, l.falloff, l.position), (1500.0, 160.0, 160.0, Vec3::ZERO));
        assert!((l.direction().len() - 1.0).abs() < 1e-12);
        assert_eq!(LightKind::from_dxf(LightKind::Spot.dxf()), LightKind::Spot);
    }
}
