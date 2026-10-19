//! Geographic location (GEOGRAPHICLOCATION): where the drawing sits on the earth. A design point
//! in the drawing is at a latitude, longitude and elevation; true north points along a direction
//! in the drawing; drawing units have a size in metres. Points convert between the drawing and
//! latitude/longitude on the WGS 84 ellipsoid, in a local tangent plane at the location (exact
//! enough across a site), and to UTM grid coordinates (the Transverse Mercator series).
//!
//! Saved in DXF as the public GEODATA object (design coordinate type 3, geographic).

use cadaddy_geom::{Vec2, Vec3};
use serde::{Deserialize, Serialize};

/// WGS 84: semi-major axis (m) and flattening.
const A: f64 = 6_378_137.0;
const F: f64 = 1.0 / 298.257_223_563;

/// A coordinate system the location is described in: geographic latitude/longitude, or a UTM
/// zone (grid coordinates in metres).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CoordSystem {
    /// Latitude/longitude on WGS 84 (`LL84`).
    Geographic,
    /// UTM zone 1–60, north or south hemisphere.
    Utm { zone: u8, north: bool },
}

impl CoordSystem {
    /// `LL84`, `UTM84-14N`; parsed back by [`CoordSystem::parse`].
    pub fn code(&self) -> String {
        match self {
            CoordSystem::Geographic => "LL84".into(),
            CoordSystem::Utm { zone, north } => format!("UTM84-{zone}{}", if *north { 'N' } else { 'S' }),
        }
    }
    /// `LL84`, `WGS84`, `UTM84-14N`, `UTM 14N`, `utm-33s` (any case).
    pub fn parse(t: &str) -> Option<CoordSystem> {
        let u: String = t.trim().to_ascii_uppercase().chars().filter(|c| !matches!(c, ' ' | '-' | '_')).collect();
        if u == "LL84" || u == "WGS84" || u == "LATLONG" || u == "LL" {
            return Some(CoordSystem::Geographic);
        }
        let rest = u.strip_prefix("UTM84").or_else(|| u.strip_prefix("UTM"))?;
        let (num, hemi) = rest.split_at(rest.len().checked_sub(1)?);
        let zone: u8 = num.parse().ok().filter(|z| (1..=60).contains(z))?;
        match hemi {
            "N" => Some(CoordSystem::Utm { zone, north: true }),
            "S" => Some(CoordSystem::Utm { zone, north: false }),
            _ => None,
        }
    }
    /// The UTM zone holding a location.
    pub fn utm_for(latitude: f64, longitude: f64) -> CoordSystem {
        let zone = (((longitude + 180.0) / 6.0).floor() as i64).clamp(0, 59) as u8 + 1;
        CoordSystem::Utm { zone, north: latitude >= 0.0 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GeoLocation {
    /// Degrees: north +, east +.
    pub latitude: f64,
    pub longitude: f64,
    /// Metres above sea level at the design point.
    pub elevation: f64,
    /// The drawing point at that location.
    pub design_point: Vec3,
    /// True north, degrees counterclockwise from the drawing's +Y.
    pub north: f64,
    /// Metres per drawing unit.
    pub unit_m: f64,
    pub system: CoordSystem,
}

impl Default for GeoLocation {
    fn default() -> Self {
        GeoLocation {
            latitude: 0.0,
            longitude: 0.0,
            elevation: 0.0,
            design_point: Vec3::ZERO,
            north: 0.0,
            unit_m: 1.0,
            system: CoordSystem::Geographic,
        }
    }
}

/// Metres per drawing unit for an INSUNITS code (unitless and unknown units: metres).
pub fn metres_per_unit(insunits: i64) -> f64 {
    match insunits {
        1 => 0.0254,
        2 => 0.3048,
        3 => 1609.344,
        4 => 0.001,
        5 => 0.01,
        7 => 1000.0,
        8 => 0.0254e-6,
        9 => 0.0254e-3,
        10 => 0.9144,
        11 => 1e-10,
        12 => 1e-9,
        13 => 1e-6,
        14 => 0.1,
        15 => 10.0,
        16 => 100.0,
        17 => 1e9,
        18 => 149_597_870_700.0,
        19 => 9.460_730_472_580_8e15,
        20 => 3.085_677_581e16,
        21 => 1200.0 / 3937.0,
        _ => 1.0,
    }
}

/// Radii of curvature (meridian, prime vertical) at latitude `phi` (radians).
fn radii(phi: f64) -> (f64, f64) {
    let e2 = F * (2.0 - F);
    let s = phi.sin();
    let w = (1.0 - e2 * s * s).sqrt();
    (A * (1.0 - e2) / (w * w * w), A / w)
}

impl GeoLocation {
    /// Whether every number is usable (a file's values are checked before they are kept).
    pub fn is_valid(&self) -> bool {
        (-90.0..=90.0).contains(&self.latitude)
            && (-180.0..=180.0).contains(&self.longitude)
            && self.elevation.is_finite()
            && self.design_point.x.is_finite()
            && self.design_point.y.is_finite()
            && self.design_point.z.is_finite()
            && self.north.is_finite()
            && self.unit_m.is_finite()
            && self.unit_m > 0.0
    }
    /// True north as a unit drawing direction.
    pub fn north_dir(&self) -> Vec2 {
        let a = self.north.to_radians();
        Vec2::new(-a.sin(), a.cos())
    }
    /// The (east, north) metres of drawing point `p` from the design point.
    fn local(&self, p: Vec2) -> (f64, f64) {
        let v = (p - Vec2::new(self.design_point.x, self.design_point.y)) * self.unit_m;
        let n = self.north_dir();
        let e = Vec2::new(n.y, -n.x);
        (v.x * e.x + v.y * e.y, v.x * n.x + v.y * n.y)
    }
    /// Latitude and longitude (degrees) of drawing point `p`.
    pub fn to_latlong(&self, p: Vec2) -> (f64, f64) {
        let (east, north) = self.local(p);
        let phi0 = self.latitude.to_radians();
        let (m, n) = radii(phi0);
        let lat = self.latitude + (north / m).to_degrees();
        let cos = phi0.cos().abs().max(1e-12);
        let lon = self.longitude + (east / (n * cos)).to_degrees();
        (lat.clamp(-90.0, 90.0), ((lon + 180.0).rem_euclid(360.0)) - 180.0)
    }
    /// The drawing point at latitude/longitude (degrees).
    pub fn from_latlong(&self, latitude: f64, longitude: f64) -> Vec2 {
        let phi0 = self.latitude.to_radians();
        let (m, n) = radii(phi0);
        let mut dlon = longitude - self.longitude;
        if dlon > 180.0 {
            dlon -= 360.0;
        } else if dlon < -180.0 {
            dlon += 360.0;
        }
        let north = (latitude - self.latitude).to_radians() * m;
        let east = dlon.to_radians() * n * phi0.cos();
        let nd = self.north_dir();
        let ed = Vec2::new(nd.y, -nd.x);
        let unit = if self.unit_m > 0.0 { self.unit_m } else { 1.0 };
        Vec2::new(self.design_point.x, self.design_point.y) + (ed * east + nd * north) * (1.0 / unit)
    }
}

/// UTM grid coordinates (easting, northing, m) of latitude/longitude (degrees) in `zone`
/// (Krüger series, mm accuracy within the zone).
pub fn utm_forward(latitude: f64, longitude: f64, zone: u8, north: bool) -> (f64, f64) {
    let (k0, n) = (0.9996, F / (2.0 - F));
    let a_big = A / (1.0 + n) * (1.0 + n * n / 4.0 + n.powi(4) / 64.0);
    let alpha = [n / 2.0 - 2.0 * n * n / 3.0 + 5.0 * n.powi(3) / 16.0, 13.0 * n * n / 48.0 - 3.0 * n.powi(3) / 5.0, 61.0 * n.powi(3) / 240.0];
    let lon0 = f64::from(zone) * 6.0 - 183.0;
    let phi = latitude.to_radians();
    let lam = (longitude - lon0).to_radians();
    let e = (F * (2.0 - F)).sqrt();
    let t = (phi.sin().atanh() - e * (e * phi.sin()).atanh()).sinh();
    let xi = t.atan2(lam.cos());
    let eta = (lam.sin() / (1.0 + t * t).sqrt()).atanh();
    let mut x = eta;
    let mut y = xi;
    for (j, a) in alpha.iter().enumerate() {
        let k = 2.0 * (j as f64 + 1.0);
        x += a * (k * xi).cos() * (k * eta).sinh();
        y += a * (k * xi).sin() * (k * eta).cosh();
    }
    let easting = 500_000.0 + k0 * a_big * x;
    let northing = k0 * a_big * y + if north { 0.0 } else { 10_000_000.0 };
    (easting, northing)
}

/// Latitude/longitude (degrees) of UTM grid coordinates in `zone`.
pub fn utm_inverse(easting: f64, northing: f64, zone: u8, north: bool) -> (f64, f64) {
    let (k0, n) = (0.9996, F / (2.0 - F));
    let a_big = A / (1.0 + n) * (1.0 + n * n / 4.0 + n.powi(4) / 64.0);
    let beta = [n / 2.0 - 2.0 * n * n / 3.0 + 37.0 * n.powi(3) / 96.0, n * n / 48.0 + n.powi(3) / 15.0, 17.0 * n.powi(3) / 480.0];
    let delta = [2.0 * n - 2.0 * n * n / 3.0 - 2.0 * n.powi(3), 7.0 * n * n / 3.0 - 8.0 * n.powi(3) / 5.0, 56.0 * n.powi(3) / 15.0];
    let lon0 = f64::from(zone) * 6.0 - 183.0;
    let xi = (northing - if north { 0.0 } else { 10_000_000.0 }) / (k0 * a_big);
    let eta = (easting - 500_000.0) / (k0 * a_big);
    let mut xi1 = xi;
    let mut eta1 = eta;
    for (j, b) in beta.iter().enumerate() {
        let k = 2.0 * (j as f64 + 1.0);
        xi1 -= b * (k * xi).sin() * (k * eta).cosh();
        eta1 -= b * (k * xi).cos() * (k * eta).sinh();
    }
    let chi = (xi1.sin() / eta1.cosh()).asin();
    let mut phi = chi;
    for (j, d) in delta.iter().enumerate() {
        phi += d * (2.0 * (j as f64 + 1.0) * chi).sin();
    }
    let lam = eta1.sinh().atan2(xi1.cos());
    (phi.to_degrees(), lon0 + lam.to_degrees())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_conversion_round_trips_and_follows_north() {
        let g = GeoLocation {
            latitude: 40.0,
            longitude: -99.0,
            design_point: Vec3::new(100.0, 200.0, 0.0),
            north: 30.0,
            unit_m: 0.3048,
            ..GeoLocation::default()
        };
        assert_eq!(g.to_latlong(Vec2::new(100.0, 200.0)), (40.0, -99.0));
        for p in [Vec2::new(1000.0, -500.0), Vec2::new(-2500.0, 4000.0)] {
            let (la, lo) = g.to_latlong(p);
            let back = g.from_latlong(la, lo);
            assert!((back - p).len() < 1e-6, "{p:?} → {back:?}");
        }
        // 1000 ft along true north is about 0.00275 degrees of latitude, no longitude.
        let p = Vec2::new(100.0, 200.0) + g.north_dir() * 1000.0;
        let (la, lo) = g.to_latlong(p);
        assert!((la - 40.0 - 0.002_751).abs() < 1e-5 && (lo + 99.0).abs() < 1e-9, "{la} {lo}");
    }

    #[test]
    fn utm_matches_known_points_and_inverts() {
        // The UTM zone 31N origin meridian (3°E) on the equator: 500 000 m east, 0 north.
        let (e, n) = utm_forward(0.0, 3.0, 31, true);
        assert!((e - 500_000.0).abs() < 1e-6 && n.abs() < 1e-6);
        for (la, lo) in [(40.0, -99.0), (-33.86, 151.21), (64.14, -21.94)] {
            let CoordSystem::Utm { zone, north } = CoordSystem::utm_for(la, lo) else { panic!() };
            let (e, n) = utm_forward(la, lo, zone, north);
            assert!((166_000.0..834_000.0).contains(&e), "{e}");
            let (la2, lo2) = utm_inverse(e, n, zone, north);
            assert!((la2 - la).abs() < 1e-8 && (lo2 - lo).abs() < 1e-8, "{la2} {lo2}");
        }
        // 45°N on a zone's central meridian: the meridian arc (4 984 944.4 m) times 0.9996.
        let (_, n) = utm_forward(45.0, 9.0, 32, true);
        assert!((n - 4_984_944.4 * 0.9996).abs() < 1.0, "{n}");
    }

    #[test]
    fn coordinate_system_codes() {
        assert_eq!(CoordSystem::parse("utm 14n"), Some(CoordSystem::Utm { zone: 14, north: true }));
        assert_eq!(CoordSystem::parse("UTM84-33S").map(|c| c.code()).as_deref(), Some("UTM84-33S"));
        assert_eq!(CoordSystem::parse("ll84"), Some(CoordSystem::Geographic));
        assert_eq!(CoordSystem::parse("UTM-61N"), None);
        assert_eq!(CoordSystem::utm_for(40.0, -99.0), CoordSystem::Utm { zone: 14, north: true });
    }
}
