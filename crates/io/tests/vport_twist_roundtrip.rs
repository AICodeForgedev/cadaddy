//! The VPORT table's view twist (group 51): every tile of a tiled or saved configuration keeps
//! its own, and an untiled model view's twist is kept as a lone `*Active` tile.

use cadaddy_doc::geom::Vec2;
use cadaddy_doc::{Drawing, VportConfig, VportTile};
use cadaddy_io::{read_dxf, write_dxf};

fn tile(min: (f64, f64), max: (f64, f64), twist_deg: f64) -> VportTile {
    VportTile { min: Vec2::new(min.0, min.1), max: Vec2::new(max.0, max.1), center: Vec2::new(1.0, 2.0), height: 4.0, aspect: 1.0, twist: twist_deg.to_radians() }
}

/// The values of group 51 in the file's VPORT table.
fn vport_51(text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    let start = lines.windows(2).position(|w| w[0] == "2" && w[1] == "VPORT").unwrap_or(lines.len());
    let end = lines.iter().skip(start).position(|l| *l == "ENDTAB").map_or(lines.len(), |e| start + e);
    let sec = lines.get(start..end).unwrap_or_default();
    sec.chunks(2).filter(|c| c.first() == Some(&"51")).filter_map(|c| c.get(1).map(|v| v.to_string())).collect()
}

fn active_twist(d: &Drawing) -> Option<f64> {
    d.vports.iter().find(|c| c.is_active()).and_then(|c| c.tiles.first()).map(|t| t.twist.to_degrees())
}

#[test]
fn each_tile_keeps_its_twist() {
    let mut d = Drawing::new_imperial();
    d.vports = vec![
        VportConfig { name: VportConfig::ACTIVE.into(), tiles: vec![tile((0.0, 0.0), (0.5, 1.0), 30.0), tile((0.5, 0.0), (1.0, 1.0), 0.0)] },
        VportConfig { name: "Pair".into(), tiles: vec![tile((0.0, 0.0), (1.0, 0.5), 0.0), tile((0.0, 0.5), (1.0, 1.0), 300.0)] },
    ];
    let r = read_dxf(write_dxf(&d).as_bytes()).unwrap();
    let twists: Vec<Vec<f64>> = r.vports.iter().map(|c| c.tiles.iter().map(|t| (t.twist.to_degrees() * 1e6).round() / 1e6).collect()).collect();
    assert_eq!(twists, vec![vec![30.0, 0.0], vec![0.0, 300.0]]);
}

#[test]
fn an_untiled_view_keeps_its_twist_as_a_lone_active_tile() {
    let mut d = Drawing::new_imperial();
    d.vports = vec![VportConfig { name: VportConfig::ACTIVE.into(), tiles: vec![tile((0.0, 0.0), (1.0, 1.0), 45.0)] }];
    let text = write_dxf(&d);
    let v = vport_51(&text);
    assert_eq!(v.len(), 1, "the *Active VPORT carries group 51");
    assert!((v[0].parse::<f64>().unwrap() - 45.0).abs() < 1e-9);
    let r = read_dxf(text.as_bytes()).unwrap();
    assert!((active_twist(&r).unwrap() - 45.0).abs() < 1e-9);
    // No twist and no tiling: no configuration, as before.
    d.vports[0].tiles[0].twist = 0.0;
    assert!(read_dxf(write_dxf(&d).as_bytes()).unwrap().vports.is_empty());
}

#[test]
fn hostile_twists_are_read_and_written_within_one_turn() {
    let mut d = Drawing::new_imperial();
    d.vports = vec![VportConfig { name: VportConfig::ACTIVE.into(), tiles: vec![tile((0.0, 0.0), (1.0, 1.0), 0.0)] }];
    d.vports[0].tiles[0].twist = 1e300;
    let text = write_dxf(&d);
    assert!(vport_51(&text).iter().all(|v| v.parse::<f64>().is_ok_and(|a| (0.0..360.0).contains(&a))), "no huge angle is written");
    d.vports[0].tiles[0].twist = f64::NAN;
    let text = write_dxf(&d);
    let r = read_dxf(text.as_bytes()).unwrap();
    assert!(r.vports.is_empty());
    // A file's 51 of 765 degrees reads as 45.
    let mut d = Drawing::new_imperial();
    d.vports = vec![VportConfig { name: VportConfig::ACTIVE.into(), tiles: vec![tile((0.0, 0.0), (1.0, 1.0), 45.0)] }];
    let text = write_dxf(&d);
    let v = vport_51(&text);
    let text = text.replacen(&format!("51\r\n{}\r\n", v[0]), "51\r\n765.0\r\n", 1);
    assert_eq!(vport_51(&text), vec!["765.0".to_string()]);
    let r = read_dxf(text.as_bytes()).unwrap();
    assert!((active_twist(&r).unwrap() - 45.0).abs() < 1e-9);
}

/// The untiled `*Active` view of a turned model view fits the extents as the turned view sees
/// them: a 100 × 10 drawing turned a quarter turn is 100 high on screen.
#[test]
fn a_turned_untiled_view_fits_the_turned_extents() {
    let mut d = Drawing::new_imperial();
    d.add(
        &cadaddy_doc::Space::Model,
        cadaddy_doc::Common::default(),
        cadaddy_doc::EntityKind::Line(cadaddy_doc::Line { a: cadaddy_doc::geom::Vec3::new(0.0, 0.0, 0.0), b: cadaddy_doc::geom::Vec3::new(100.0, 10.0, 0.0) }),
    )
    .unwrap();
    d.vports = vec![VportConfig { name: VportConfig::ACTIVE.into(), tiles: vec![tile((0.0, 0.0), (1.0, 1.0), 90.0)] }];
    let r = read_dxf(write_dxf(&d).as_bytes()).unwrap();
    let t = r.vports.iter().find(|c| c.is_active()).and_then(|c| c.tiles.first()).copied().unwrap();
    assert!((t.height - 110.0).abs() < 1e-6, "{t:?}");
    assert!((t.center - Vec2::new(50.0, 5.0)).len() < 1e-6, "{t:?}");
}
