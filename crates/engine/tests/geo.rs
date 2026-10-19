//! Geographic location (GEOGRAPHICLOCATION, LATLONG, GEOREMOVE): set by JSON and by prompts,
//! points converted both ways (and to UTM), the sun placed, LATLONG at a point prompt, and the
//! location kept in DXF (GEODATA) and .cdy files.

use cadaddy_engine::doc::EntityKind;
use cadaddy_engine::doc::geo::CoordSystem;
use cadaddy_engine::{Input, Session};
use serde_json::json;

fn located() -> Session {
    let mut s = Session::new();
    s.execute(
        "geographiclocation",
        &json!({ "latitude": 40.0005, "longitude": -99.0004, "elevation": 199.0, "point": [100, 50], "north": 15, "unit": 0.3048, "system": "utm" }),
    )
    .unwrap();
    s
}

#[test]
fn set_convert_and_place_the_sun() {
    let mut s = located();
    let g = s.doc().unwrap().geo.clone().unwrap();
    assert_eq!(g.system, CoordSystem::Utm { zone: 14, north: true });
    let sun = &s.doc().unwrap().sun;
    assert_eq!((sun.latitude, sun.longitude, sun.north), (40.0005, -99.0004, 15.0));
    // The design point is the location itself.
    let r = s.execute("latlong", &json!({ "point": [100, 50] })).unwrap();
    assert!((r["latitude"].as_f64().unwrap() - 40.0005).abs() < 1e-12 && (r["longitude"].as_f64().unwrap() + 99.0004).abs() < 1e-12);
    let e = r["easting"].as_f64().unwrap();
    assert!((e - 500_000.0).abs() < 3_000.0, "UTM 14N easting near the central meridian: {e}");
    // A point 500 ft away converts there and back.
    let r = s.execute("latlong", &json!({ "point": [600, -200] })).unwrap();
    let (la, lo) = (r["latitude"].as_f64().unwrap(), r["longitude"].as_f64().unwrap());
    let back = s.execute("latlong", &json!({ "latitude": la, "longitude": lo })).unwrap();
    let p = back["point"].as_array().unwrap();
    assert!((p[0].as_f64().unwrap() - 600.0).abs() < 1e-6 && (p[1].as_f64().unwrap() + 200.0).abs() < 1e-6, "{back}");
    // From UTM grid coordinates too.
    let (e, n) = (r["easting"].as_f64().unwrap(), r["northing"].as_f64().unwrap());
    let g2 = s.execute("latlong", &json!({ "easting": e, "northing": n })).unwrap();
    let q = g2["point"].as_array().unwrap();
    // Through the UTM projection and back: within a few millimetres.
    assert!((q[0].as_f64().unwrap() - 600.0).abs() < 0.01 && (q[1].as_f64().unwrap() + 200.0).abs() < 0.01, "{g2}");
    // Changing one value keeps the rest; `sun: false` leaves the sun.
    s.execute("geographiclocation", &json!({ "north": 0, "sun": false })).unwrap();
    assert_eq!(s.doc().unwrap().geo.as_ref().unwrap().latitude, 40.0005);
    assert_eq!(s.doc().unwrap().sun.north, 15.0);
    for bad in [json!({ "latitude": 91, "longitude": 0 }), json!({ "system": "UTM-99N" }), json!({ "unit": 0 }), json!({ "northDirection": [0, 0] })]
    {
        assert!(s.execute("geographiclocation", &bad).is_err(), "{bad}");
    }
    assert!(Session::new().execute("geographiclocation", &json!({ "latitude": 10 })).is_err(), "both are needed at first");
    s.execute("georemove", &json!({})).unwrap();
    assert!(s.doc().unwrap().geo.is_none());
    assert!(s.execute("latlong", &json!({ "point": [0, 0] })).is_err());
    assert_eq!(s.execute("georemove", &json!({})).unwrap()["removed"], false, "nothing to remove");
}

#[test]
fn prompts_and_latlong_point_input() {
    let mut s = Session::new();
    s.cmdline("geographiclocation").unwrap();
    s.cmdline("40.0").unwrap();
    s.cmdline("-105.0").unwrap();
    s.input(Input::Point(cadaddy_engine::geom::Vec2::new(10.0, 10.0))).unwrap();
    s.cmdline("90").unwrap();
    assert!(s.running.is_none(), "{}", s.prompt_text());
    let g = s.doc().unwrap().geo.clone().unwrap();
    assert_eq!((g.latitude, g.longitude, g.north), (40.0, -105.0, 90.0));
    // LINE from the design point to a typed latitude/longitude a little north of it.
    s.cmdline("line").unwrap();
    s.cmdline("10,10").unwrap();
    s.cmdline("latlong").unwrap();
    assert!(s.prompt_text().contains("latitude,longitude"), "{}", s.prompt_text());
    s.cmdline("40.001,-105.0").unwrap();
    s.cmdline("").unwrap();
    let EntityKind::Line(l) = &s.doc().unwrap().model.iter().last().unwrap().kind else { panic!("no line") };
    // North is 90° counterclockwise from +Y: the drawing's -X. 0.001° of latitude is ~111 m,
    // in the drawing's units (INSUNITS).
    assert!((l.b.y - 10.0).abs() < 1e-6, "{:?}", l.b);
    assert!(((10.0 - l.b.x) * g.unit_m - 111.0).abs() < 0.5, "{:?} at {} m per unit", l.b, g.unit_m);
}

#[test]
fn geodata_survives_dxf_and_cdy() {
    let s = located();
    let d = s.doc().unwrap();
    let want = d.geo.clone().unwrap();
    let dxf = cadaddy_io::write(d, "site.dxf").unwrap();
    let text = String::from_utf8_lossy(&dxf);
    assert!(text.contains("GEODATA") && text.contains("ACAD_GEOGRAPHICDATA") && text.contains("AcDbGeoData"));
    let back = cadaddy_io::read(&dxf, "site.dxf").unwrap();
    let g = back.geo.expect("GEODATA read back");
    assert!((g.latitude - want.latitude).abs() < 1e-9 && (g.longitude - want.longitude).abs() < 1e-9);
    assert!((g.north - want.north).abs() < 1e-9 && (g.unit_m - want.unit_m).abs() < 1e-12, "{g:?}");
    assert_eq!((g.design_point, g.elevation, g.system.clone()), (want.design_point, want.elevation, want.system.clone()));
    let cdy = cadaddy_io::write(d, "site.cdy").unwrap();
    assert_eq!(cadaddy_io::read(&cdy, "site.cdy").unwrap().geo, Some(want));
    // A drawing without a location writes none.
    let plain = cadaddy_io::write(&cadaddy_engine::doc::Drawing::new_metric(), "p.dxf").unwrap();
    assert!(!String::from_utf8_lossy(&plain).contains("GEODATA"));
}
