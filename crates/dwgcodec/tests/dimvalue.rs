//! Dimension kinds through the native encoder and decoder: the measurement and the defining
//! points of every kind come back unchanged in every DWG version.

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
    let names = Handles::default();
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
    let raw = ek::entity(v, "DIMENSION", 0x50, &Rec(groups), &head, &names, &[], None).expect("encoded");
    let classes = Classes::new(v, vec![]);
    entities::decode(&raw, v, &classes, &Names::default()).expect("decodes").expect("entity").groups
}

fn num(groups: &[Group], code: i32) -> Vec<f64> {
    groups.iter().filter(|g| g.code == code).filter_map(|g| if let Value::Real(f) = g.value { Some(f) } else { None }).collect()
}

#[test]
fn every_kind_keeps_its_measurement_and_points_in_every_version() {
    // (DXF dimension type, points by group code)
    let kinds: [(i64, Vec<(i32, [f64; 3])>); 4] = [
        (4, vec![(10, [10.0, 10.0, 0.0]), (15, [24.79, 10.0, 0.0])]),
        (3, vec![(10, [10.0, 10.0, 0.0]), (15, [30.0, 10.0, 0.0])]),
        (5, vec![(10, [1.0, 1.0, 0.0]), (13, [10.0, 0.0, 0.0]), (14, [0.0, 10.0, 0.0]), (15, [3.0, 3.0, 0.0])]),
        (2, vec![(10, [1.0, 2.0, 0.0]), (13, [10.0, 0.0, 0.0]), (14, [0.0, 10.0, 0.0]), (15, [3.0, 3.0, 0.0]), (16, [5.0, 5.0, 0.0])]),
    ];
    for (kind, pts) in kinds {
        let mut g = vec![gi(70, kind), gf(42, 7.395)];
        g.extend(gp(11, [5.0, 6.0, 0.0]));
        for (c, p) in &pts {
            g.extend(gp(*c, *p));
        }
        for v in VERSIONS {
            let b = back(v, &g);
            assert_eq!(num(&b, 42), vec![7.395], "{kind} {v:?}: measurement");
            for (c, p) in &pts {
                assert_eq!(num(&b, *c), vec![p[0]], "{kind} {v:?}: point {c} x");
                assert_eq!(num(&b, *c + 10), vec![p[1]], "{kind} {v:?}: point {c} y");
            }
        }
    }
}
