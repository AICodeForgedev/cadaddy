//! Clipped, locked and turned-off layout viewports survive a DXF round trip (#453): VIEWPORT
//! status flags (group 90: 16384 locked, 65536 non-rectangular clipping, 131072 off), status 68
//! (0 = off) and the clip boundary entity (group 340), per the public DXF Reference.

use cadaddy_doc::*;
use cadaddy_geom::{PolyVertex, Vec2, Vec3};

/// A metric drawing with model lines and, on its first layout, an L-shaped closed polyline
/// clipping a viewport.
fn drawing(locked: bool, on: bool) -> (Drawing, Handle, Handle) {
    let mut d = Drawing::new_metric();
    for i in 0..=10 {
        let y = f64::from(i) * 10.0;
        d.add(&Space::Model, Common::default(), EntityKind::Line(Line { a: Vec3::new(0.0, y, 0.0), b: Vec3::new(100.0, y, 0.0) })).unwrap();
    }
    let sheet = Space::Paper(d.layouts[0].name.clone());
    let l = [[10.0, 10.0], [110.0, 10.0], [110.0, 60.0], [60.0, 60.0], [60.0, 110.0], [10.0, 110.0]];
    let poly = LwPolyline {
        vertices: l.iter().map(|p| PolyVertex::new(Vec2::new(p[0], p[1]))).collect(),
        closed: true,
        const_width: 0.0,
        elevation: 0.0,
        plinegen: false,
    };
    let b = d.add(&sheet, Common::default(), EntityKind::LwPolyline(poly)).unwrap();
    let vp = Viewport {
        center: Vec3::new(60.0, 60.0, 0.0),
        width: 100.0,
        height: 100.0,
        view_center: Vec2::new(50.0, 50.0),
        view_height: 100.0,
        id: 2,
        locked,
        clip: Some(b),
        on,
        ..Viewport::default()
    };
    let v = d.add(&sheet, Common::default(), EntityKind::Viewport(vp)).unwrap();
    (d, v, b)
}

/// The DXF text as (code, value) pairs, and back.
fn groups(text: &str) -> Vec<(String, String)> {
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    lines.chunks(2).filter_map(|c| Some((c.first()?.to_string(), c.get(1)?.to_string()))).collect()
}
fn text_of(gs: &[(String, String)]) -> String {
    gs.iter().map(|(c, v)| format!("{c}\n{v}\n")).collect()
}

fn viewport(d: &Drawing, h: Handle) -> Viewport {
    match d.entity(h).map(|e| e.kind.clone()) {
        Some(EntityKind::Viewport(v)) => v,
        other => panic!("not a viewport: {other:?}"),
    }
}

#[test]
fn clip_boundary_lock_and_off_roundtrip() {
    for (locked, on) in [(false, true), (true, false), (true, true)] {
        let (d, v, b) = drawing(locked, on);
        let text = cadaddy_io::write_dxf(&d);
        let back = cadaddy_io::read_dxf(text.as_bytes()).unwrap();
        let got = viewport(&back, v);
        assert_eq!(got.clip, Some(b));
        assert_eq!((got.locked, got.on), (locked, on));
        assert!(matches!(back.entity(b).map(|e| &e.kind), Some(EntityKind::LwPolyline(p)) if p.closed && p.vertices.len() == 6));
        // The flags as the DXF Reference has them.
        let flags = 32768 + 65536 + if locked { 16384 } else { 0 } + if on { 0 } else { 131072 };
        let gs = groups(&text);
        assert!(gs.contains(&("90".into(), flags.to_string())), "flags {flags}");
        assert!(gs.contains(&("340".into(), b.hex())));
    }
}

#[test]
fn clipped_viewport_plots_through_its_outline() {
    let (d, _, _) = drawing(false, true);
    let back = cadaddy_io::read_dxf(cadaddy_io::write_dxf(&d).as_bytes()).unwrap();
    let sheet = Space::Paper(back.layouts[0].name.clone());
    // PNG, SVG and PDF all draw from this list.
    let l = cadaddy_render::build_plot(&back, &sheet, &cadaddy_render::Options::default());
    let pts: Vec<Vec2> = l.prims.iter().filter(|p| p.handle == cadaddy_render::VIEWPORT_CONTENT).flat_map(|p| l.points(p).to_vec()).collect();
    assert!(!pts.is_empty());
    assert!(pts.iter().all(|p| !(p.x > 60.0 + 1e-6 && p.y > 60.0 + 1e-6)), "content in the notch");
    for name in ["out.svg", "out.pdf", "out.png"] {
        assert!(!cadaddy_io::write(&back, name).unwrap().is_empty(), "{name}");
    }
}

#[test]
fn hostile_clip_handles_do_not_panic() {
    let (d, v, b) = drawing(false, true);
    let gs = groups(&cadaddy_io::write_dxf(&d));
    let at = gs.iter().position(|g| *g == ("340".to_string(), b.hex())).unwrap();
    for bad in ["0", "ZZZ", "", "FFFFFFFFFFFFFFFFFFFF", "-1", "7FFFFFFFFFFFFFFF", &v.hex()] {
        let mut g = gs.clone();
        g[at].1 = bad.to_string();
        let back = cadaddy_io::read_dxf(text_of(&g).as_bytes()).unwrap();
        let got = viewport(&back, v);
        let sheet = Space::Paper(back.layouts[0].name.clone());
        // A boundary that isn't there clips nothing: the rectangle shows model space.
        let l = cadaddy_render::build(&back, &sheet, &cadaddy_render::Options::default());
        assert!(l.prims.iter().any(|p| p.handle == cadaddy_render::VIEWPORT_CONTENT), "{bad}");
        // And it writes again without a dangling 340.
        if got.clip.is_none_or(|h| back.entity(h).is_none() || h == v) {
            let again = groups(&cadaddy_io::write_dxf(&back));
            let start = again.iter().position(|g| *g == ("5".to_string(), v.hex())).unwrap();
            let record = again[start..].iter().take_while(|(c, _)| c != "0");
            assert!(!record.into_iter().any(|(c, _)| c == "340"), "{bad}");
        }
    }
    // The clipping flag without a 340 group: a rectangular viewport.
    let mut g = gs.clone();
    g.remove(at);
    assert_eq!(viewport(&cadaddy_io::read_dxf(text_of(&g).as_bytes()).unwrap(), v).clip, None);
}

/// The view twist (group 51, degrees) round-trips, and the view centre (12/22, in display
/// coordinates, which the twist turns) comes back as the same model point.
#[test]
fn view_twist_roundtrips() {
    let (mut d, v, _) = drawing(false, true);
    let twist = 30f64.to_radians();
    let sheet = Space::Paper(d.layouts[0].name.clone());
    d.space_mut(&sheet).unwrap().modify(v, |e| {
        if let EntityKind::Viewport(vp) = &mut e.kind {
            vp.twist = twist;
        }
    });
    let text = cadaddy_io::write_dxf(&d);
    let gs = groups(&text);
    let at = gs.iter().position(|g| *g == ("5".to_string(), v.hex())).unwrap();
    let record: Vec<&(String, String)> = gs[at..].iter().take_while(|(c, _)| c != "0").collect();
    let deg = record.iter().find(|(c, _)| c == "51").map(|(_, x)| x.parse::<f64>().unwrap()).unwrap();
    assert!((deg - 30.0).abs() < 1e-9);
    // Display coordinates: the model point 50,50 turned by the twist.
    let x = record.iter().find(|(c, _)| c == "12").map(|(_, x)| x.parse::<f64>().unwrap()).unwrap();
    assert!((x - Vec2::new(50.0, 50.0).rotate(twist).x).abs() < 1e-9);
    let got = viewport(&cadaddy_io::read_dxf(text.as_bytes()).unwrap(), v);
    assert!((got.twist - twist).abs() < 1e-9);
    assert!(got.view_center.near(Vec2::new(50.0, 50.0), 1e-9), "{:?}", got.view_center);
    // An untwisted viewport writes no 51; hostile values read as no twist.
    let (d0, v0, _) = drawing(false, true);
    let g0 = groups(&cadaddy_io::write_dxf(&d0));
    let at0 = g0.iter().position(|g| *g == ("5".to_string(), v0.hex())).unwrap();
    assert!(!g0[at0..].iter().take_while(|(c, _)| c != "0").any(|(c, _)| c == "51"));
    for bad in ["nan", "inf", "-inf", "1e400", "x"] {
        let mut g = gs.clone();
        if let Some(i) = g.iter().skip(at).position(|(c, _)| c == "51") {
            g[at + i].1 = bad.to_string();
        }
        let back = cadaddy_io::read_dxf(text_of(&g).as_bytes()).unwrap();
        let got = viewport(&back, v);
        assert!(got.twist.is_finite() && got.view_center.is_finite(), "{bad}");
    }
}
