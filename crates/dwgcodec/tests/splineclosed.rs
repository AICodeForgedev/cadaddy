//! Closed and periodic splines through the native encoder and decoder: the closed and periodic
//! flags, the control points, the knots and the weights come back unchanged in every DWG
//! version, with or without fit points. Open fit splines still come back as fit splines.

use cadaddy_dwgcodec::Version;
use cadaddy_dwgcodec::classes::Classes;
use cadaddy_dwgcodec::entities;
use cadaddy_dwgcodec::entities::encode::EntityHead;
use cadaddy_dwgcodec::entities::encode::{Handles, Rec};
use cadaddy_dwgcodec::entities::encode_kinds as ek;
use cadaddy_dwgcodec::entities::{Group, Names, Value};

const VERSIONS: [Version; 6] = [Version::R2000, Version::R2004, Version::R2007, Version::R2010, Version::R2013, Version::R2018];

fn gi(code: i32, i: i64) -> Group {
    Group { code, value: Value::Int(i) }
}
fn gf(code: i32, f: f64) -> Group {
    Group { code, value: Value::Real(f) }
}
fn gp(code: i32, p: [f64; 3]) -> Vec<Group> {
    vec![gf(code, p[0]), gf(code + 10, p[1]), gf(code + 20, p[2])]
}

fn back(v: Version, groups: &[Group]) -> Vec<Group> {
    let head = EntityHead {
        entmode: 2,
        owner: None,
        reactors: vec![],
        xdict: None,
        layer: None,
        ltype: None,
        ltype_flags: 0,
        material: None,
        plotstyle: None,
        links: None,
    };
    let raw = ek::entity(v, "SPLINE", 0x50, &Rec(groups), &head, &Handles::default(), &[], None).expect("encoded");
    let classes = Classes::new(v, vec![]);
    entities::decode(&raw, v, &classes, &Names::default()).expect("decodes").expect("entity").groups
}

fn reals(groups: &[Group], code: i32) -> Vec<f64> {
    groups.iter().filter(|g| g.code == code).filter_map(|g| if let Value::Real(f) = g.value { Some(f) } else { None }).collect()
}
fn int(groups: &[Group], code: i32) -> Option<i64> {
    groups.iter().find(|g| g.code == code).and_then(|g| if let Value::Int(i) = g.value { Some(i) } else { None })
}

/// A degree-3 periodic spline: `n` control points round a circle, the wrapped uniform knots.
fn periodic(n: usize, fits: bool) -> Vec<Group> {
    let mut g = vec![gi(70, 1 | 2), gi(71, 3)];
    let knots = n + 3 + 1;
    g.push(gi(72, knots as i64));
    g.push(gi(73, n as i64));
    for k in 0..knots {
        g.push(gf(40, k as f64 - 3.0));
    }
    for i in 0..n {
        let a = std::f64::consts::TAU * i as f64 / n as f64;
        g.extend(gp(10, [5.0 + 3.0 * a.cos(), 2.0 + 2.0 * a.sin(), 0.0]));
    }
    if fits {
        g.push(gi(74, 8));
        for i in 0..8 {
            let a = std::f64::consts::TAU * i as f64 / 8.0;
            g.extend(gp(11, [5.0 + 3.0 * a.cos(), 2.0 + 2.0 * a.sin(), 0.0]));
        }
    }
    g
}

fn same_curve(v: Version, input: &[Group], b: &[Group], label: &str) {
    assert_eq!(int(b, 70).map(|f| f & 3), int(input, 70).map(|f| f & 3), "{label} {v:?}: closed/periodic flags");
    assert_eq!(reals(b, 40), reals(input, 40), "{label} {v:?}: knots");
    for code in [10, 20, 30] {
        assert_eq!(reals(b, code), reals(input, code), "{label} {v:?}: control {code}");
    }
}

#[test]
fn a_periodic_spline_keeps_flags_controls_and_knots_in_every_version() {
    for v in VERSIONS {
        let g = periodic(131, false);
        same_curve(v, &g, &back(v, &g), "plain");
    }
}

#[test]
fn a_periodic_fit_spline_keeps_flags_controls_and_knots_in_every_version() {
    // What OFFSET of an ellipse makes: control points and knots with the fit points beside them.
    for v in VERSIONS {
        let g = periodic(131, true);
        same_curve(v, &g, &back(v, &g), "fit");
    }
}

#[test]
fn a_closed_weighted_spline_keeps_its_weights_in_every_version() {
    for v in VERSIONS {
        let mut g = periodic(12, true);
        g[0] = gi(70, 1 | 4);
        for i in 0..12 {
            g.push(gf(41, 1.0 + i as f64 / 10.0));
        }
        let b = back(v, &g);
        same_curve(v, &g, &b, "weighted");
        assert_eq!(reals(&b, 41), reals(&g, 41), "{v:?}: weights");
    }
}

#[test]
fn an_open_fit_spline_still_comes_back_as_fit_points() {
    for v in VERSIONS {
        let mut g = vec![gi(70, 1024), gi(71, 3), gi(74, 3)];
        for p in [[0.0, 0.0, 0.0], [4.0, 3.0, 0.0], [9.0, 0.0, 0.0]] {
            g.extend(gp(11, p));
        }
        let b = back(v, &g);
        assert_eq!(reals(&b, 11), vec![0.0, 4.0, 9.0], "{v:?}: fit x");
        assert_eq!(int(&b, 70).map(|f| f & 3), Some(0), "{v:?}: stays open");
    }
}
