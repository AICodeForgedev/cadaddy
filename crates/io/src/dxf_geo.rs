//! The drawing's geographic location as the public DXF GEODATA object (DXF Reference, OBJECTS
//! section): in the extension dictionary of the model space block record under
//! `ACAD_GEOGRAPHICDATA`, design coordinate type 3 (geographic), the design point (10) at the
//! reference point (11: longitude, latitude, elevation), the north direction (12), and the
//! horizontal units (40 scale from metres, 91 INSUNITS). Group 301 carries the coordinate
//! system's code (`LL84`, `UTM84-14N`).

use cadaddy_doc::Drawing;
use cadaddy_doc::geo::{CoordSystem, GeoLocation};
use cadaddy_dxf::Tag;
use cadaddy_geom::Vec3;

/// The dictionary entry naming the GEODATA object.
pub(crate) const ENTRY: &str = "ACAD_GEOGRAPHICDATA";

/// The extension dictionary (owned by model space's block record `ms_br`) and the GEODATA object.
pub(crate) fn objects(d: &Drawing, xdict: &str, geodata: &str, ms_br: &str) -> Vec<Tag> {
    let Some(g) = d.geo.as_ref().filter(|g| g.is_valid()) else { return Vec::new() };
    let mut t = vec![
        Tag::s(0, "DICTIONARY"),
        Tag::s(5, xdict),
        Tag::s(330, ms_br),
        Tag::s(100, "AcDbDictionary"),
        Tag::i(280, 1),
        Tag::i(281, 1),
        Tag::s(3, ENTRY),
        Tag::s(360, geodata),
    ];
    let north = g.north_dir();
    t.extend([
        Tag::s(0, "GEODATA"),
        Tag::s(5, geodata),
        Tag::s(102, "{ACAD_REACTORS"),
        Tag::s(330, xdict),
        Tag::s(102, "}"),
        Tag::s(330, xdict),
        Tag::s(100, "AcDbGeoData"),
        Tag::i(90, 3),
        Tag::s(330, ms_br),
        Tag::i(70, 3),
        Tag::f(10, g.design_point.x),
        Tag::f(20, g.design_point.y),
        Tag::f(30, g.design_point.z),
        Tag::f(11, g.longitude),
        Tag::f(21, g.latitude),
        Tag::f(31, g.elevation),
        Tag::f(40, 1.0 / g.unit_m),
        Tag::i(91, units_code(g.unit_m)),
        Tag::f(41, 1.0 / g.unit_m),
        Tag::i(92, units_code(g.unit_m)),
        Tag::f(210, 0.0),
        Tag::f(220, 0.0),
        Tag::f(230, 1.0),
        Tag::f(12, north.x),
        Tag::f(22, north.y),
        Tag::i(95, 1),
        Tag::f(141, 1.0),
        Tag::i(294, 0),
        Tag::f(142, 0.0),
        Tag::f(143, 0.0),
        Tag::s(301, g.system.code()),
        Tag::s(302, ""),
        Tag::s(305, ""),
        Tag::s(306, ""),
        Tag::s(307, ""),
        Tag::i(93, 0),
        Tag::i(96, 0),
    ]);
    t
}

/// The INSUNITS code whose unit is `m` metres (0 when none is).
fn units_code(m: f64) -> i64 {
    (1..=21).find(|c| (cadaddy_doc::geo::metres_per_unit(*c) - m).abs() <= m * 1e-9).unwrap_or(0)
}

fn f(tags: &[Tag], code: i32) -> Option<f64> {
    tags.iter().find(|t| t.code == code).and_then(|t| t.str().trim().parse::<f64>().ok()).filter(|v| v.is_finite())
}

/// A GEODATA object's location, when it is geographic (type 3) and its numbers make sense.
pub(crate) fn read(tags: &[Tag]) -> Option<GeoLocation> {
    // Groups before the AcDbGeoData marker are the object's own (handle, owner).
    let start = tags.iter().position(|t| t.code == 100 && t.str().trim() == "AcDbGeoData")?;
    let tags = tags.get(start..)?;
    let kind = tags.iter().find(|t| t.code == 70).and_then(|t| t.str().trim().parse::<i64>().ok()).unwrap_or(3);
    if kind != 3 {
        return None;
    }
    let design_point = Vec3::new(f(tags, 10).unwrap_or(0.0), f(tags, 20).unwrap_or(0.0), f(tags, 30).unwrap_or(0.0));
    let (longitude, latitude, elevation) = (f(tags, 11)?, f(tags, 21)?, f(tags, 31).unwrap_or(0.0));
    let unit_m = match f(tags, 40).filter(|s| *s > 0.0) {
        Some(s) => 1.0 / s,
        None => cadaddy_doc::geo::metres_per_unit(tags.iter().find(|t| t.code == 91).and_then(|t| t.str().trim().parse().ok()).unwrap_or(6)),
    };
    let (nx, ny) = (f(tags, 12).unwrap_or(0.0), f(tags, 22).unwrap_or(1.0));
    let north = if nx == 0.0 && ny == 0.0 { 0.0 } else { (-nx).atan2(ny).to_degrees() };
    let system = tags
        .iter()
        .find(|t| t.code == 301)
        .map(|t| t.str())
        .and_then(|s| {
            // Our code, or a definition naming one of the systems CADaddy knows.
            CoordSystem::parse(&s)
                .or_else(|| s.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-')).filter(|w| w.len() <= 16).find_map(CoordSystem::parse))
        })
        .unwrap_or(CoordSystem::Geographic);
    let g = GeoLocation { latitude, longitude, elevation, design_point, north: north.rem_euclid(360.0), unit_m, system };
    g.is_valid().then_some(g)
}
