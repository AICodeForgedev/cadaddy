//! Wall assemblies in the building spec (Q293): a wall type's layers, height rule and ratings, and
//! the walls the plans tag. `model.build` gives each tagged wall its type and height; IFC export
//! writes each type's IfcMaterialLayerSet (one IfcMaterialLayer per board, stud and insulation
//! layer) through IfcMaterialLayerSetUsage, and Pset_WallCommon.

use std::collections::HashMap;

use cadaddy_engine::Session;
use cadaddy_engine::doc::EntityKind;
use serde_json::{Value, json};

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("line", &json!({"points": [a, b]})).unwrap();
}

fn rect(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("pline", &json!({"vertices": [a, [b[0], a[1]], b, [a[0], b[1]]], "closed": true})).unwrap();
}

/// A made-up 40' × 24' room pair (inches): exterior walls 7 1/4" thick, a 4 7/8" partition down
/// the middle; the partition tagged `P1` with its height, the outside walls typed by their layer.
fn building(marks: bool) -> Session {
    let mut s = Session::new();
    layer(&mut s, "A-WALL");
    let t = 7.25;
    rect(&mut s, [0.0, 0.0], [480.0, 288.0]);
    rect(&mut s, [t, t], [480.0 - t, 288.0 - t]);
    let h = 4.875 / 2.0;
    line(&mut s, [240.0 - h, t], [240.0 - h, 288.0 - t]);
    line(&mut s, [240.0 + h, t], [240.0 + h, 288.0 - t]);
    let layers = |side1: Value, core: Value, side2: Value| -> Value {
        let mut v: Vec<Value> = Vec::new();
        for (side, list) in [("side1", side1), ("core", core), ("side2", side2)] {
            for l in list.as_array().cloned().unwrap_or_default() {
                let mut l = l;
                l["side"] = json!(side);
                v.push(l);
            }
        }
        json!(v)
    };
    let exterior = json!({
        "tag": "X1", "description": "EXTERIOR WALL", "height": "full", "external": true,
        "layers": layers(
            json!([{"material": "5/8\" exterior sheathing", "thickness_in": 0.625, "category": "sheathing"}]),
            json!([{"material": "6\" 18 ga metal studs @ 16\" o.c.", "thickness_in": 6.0, "category": "framing"},
                   {"material": "6\" batt insulation", "thickness_in": 6.0, "category": "insulation", "in_cavity": true}]),
            json!([{"material": "5/8\" Type X gypsum board", "thickness_in": 0.625, "category": "board"}]))
    });
    let partition = json!({
        "tag": "P1", "description": "1 HOUR RATED CORRIDOR PARTITION", "height": "deck", "fire_rating": "1 HR", "acoustic_rating": "STC 45",
        "layers": layers(
            json!([{"material": "5/8\" Type X gypsum board", "thickness_in": 0.625, "category": "board"}]),
            json!([{"material": "3 5/8\" 20 ga metal studs @ 16\" o.c.", "thickness_in": 3.625, "category": "framing"},
                   {"material": "3 1/2\" batt insulation", "thickness_in": 3.5, "category": "insulation", "in_cavity": true}]),
            json!([{"material": "5/8\" Type X gypsum board", "thickness_in": 0.625, "category": "board"}]))
    });
    let mut spec = json!({
        "version": 1,
        "units_per_foot": 12,
        "levels": [{"name": "1", "elevation_ft": 0, "height_ft": 14, "ceiling_ft": 9}],
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "X1"}],
        "wall_types": [
            {"name": "X1", "thickness_in": 7.25, "from": "pairs", "assembly": exterior},
            {"name": "P1", "thickness_in": 4.875, "from": "pairs", "assembly": partition}
        ]
    });
    if marks {
        spec["walls"] = json!([{"level": "1", "a": [240.0, 7.25], "b": [240.0, 280.75], "wall_type": "P1", "height_ft": 12.75}]);
    }
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    s
}

/// WALLGEN's wall meshes: their wall type and top (inches).
fn walls(s: &Session) -> Vec<(Option<String>, f64)> {
    let mut out = Vec::new();
    for e in s.doc().unwrap().model.iter() {
        let EntityKind::Mesh(m) = &e.kind else { continue };
        let x = &e.common.xdata;
        let Some(at) = x.iter().position(|t| t.code == 1001 && t.value == "CADADDY_WALLGEN") else { continue };
        let tags: Vec<&str> = x.iter().skip(at + 1).take_while(|t| t.code != 1001).map(|t| t.value.as_str()).collect();
        if tags.first() != Some(&"wall") {
            continue;
        }
        let ty = tags.iter().find_map(|v| v.strip_prefix("type:")).map(str::to_string);
        out.push((ty, m.vertices.iter().map(|v| v.z).fold(f64::MIN, f64::max)));
    }
    out
}

#[test]
fn tagged_walls_take_their_type_and_height() {
    let s = building(true);
    let w = walls(&s);
    assert_eq!(w.len(), 5, "{w:?}");
    let p: Vec<_> = w.iter().filter(|(t, _)| t.as_deref() == Some("P1")).collect();
    assert_eq!(p.len(), 1, "{w:?}");
    assert!((p[0].1 - 12.75 * 12.0).abs() < 1e-6, "the mark's height: {}", p[0].1);
    assert_eq!(w.iter().filter(|(t, _)| t.as_deref() == Some("X1")).count(), 4, "the others by their layer: {w:?}");
    // Without marks every wall is the layer's type.
    let s = building(false);
    assert!(walls(&s).iter().all(|(t, _)| t.as_deref() == Some("X1")));
}

/// STEP instances by number: (entity, raw arguments).
fn instances(text: &str) -> HashMap<usize, (String, String)> {
    let mut out = HashMap::new();
    for l in text.lines() {
        let Some(rest) = l.strip_prefix('#') else { continue };
        let Some((n, body)) = rest.split_once('=') else { continue };
        let Some((name, args)) = body.split_once('(') else { continue };
        out.insert(n.parse().unwrap(), (name.to_string(), args.trim_end_matches(';').trim_end_matches(')').to_string()));
    }
    out
}

fn refs(s: &str) -> Vec<usize> {
    s.split('#').skip(1).filter_map(|p| p.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()).collect()
}

#[test]
fn ifc_walls_carry_material_layers_and_wall_common() {
    let s = building(true);
    let bytes = cadaddy_io::ifc::write_ifc(s.doc().unwrap(), "walls.ifc").unwrap();
    let text = String::from_utf8(bytes).unwrap();
    let all = instances(&text);
    let of = |e: &str| all.iter().filter(|(_, (n, _))| n == e).map(|(i, (_, a))| (*i, a.clone())).collect::<Vec<_>>();
    // Every reference resolves.
    for (i, (_, a)) in &all {
        for r in refs(a) {
            assert!(all.contains_key(&r), "#{i} refers to #{r}, which isn't there");
        }
    }
    let walls: Vec<usize> = of("IFCWALL").into_iter().map(|(i, _)| i).collect();
    assert_eq!(walls.len(), 5);
    // One usage per type, each over a layer set of its four layers.
    let usages = of("IFCMATERIALLAYERSETUSAGE");
    assert_eq!(usages.len(), 2);
    for (_, u) in &usages {
        assert!(u.contains(".AXIS2.,.POSITIVE.,"), "{u}");
        let set = refs(u)[0];
        let (name, args) = &all[&set];
        assert_eq!(name, "IFCMATERIALLAYERSET");
        let layers: Vec<usize> = refs(args);
        assert_eq!(layers.len(), 4, "{args}");
        // In metres (an inch drawing): the layers add up to the wall's thickness.
        let total: f64 = layers.iter().map(|l| all[l].1.split(',').nth(1).unwrap().parse::<f64>().unwrap()).sum();
        assert!((total - 7.25 * 0.0254).abs() < 1e-6 || (total - 4.875 * 0.0254).abs() < 1e-6, "{total}");
        for l in layers {
            assert_eq!(all[&l].0, "IFCMATERIALLAYER");
            assert_eq!(all[&refs(&all[&l].1)[0]].0, "IFCMATERIAL");
        }
    }
    // Each wall associated with exactly one usage.
    let assoc = of("IFCRELASSOCIATESMATERIAL");
    assert_eq!(assoc.len(), 2);
    let mut seen: Vec<usize> = Vec::new();
    for (_, a) in &assoc {
        let r = refs(a);
        let (objs, usage) = r.split_at(r.len() - 1);
        assert_eq!(all[&usage[0]].0, "IFCMATERIALLAYERSETUSAGE");
        for o in objs {
            assert!(walls.contains(o));
            assert!(!seen.contains(o), "a wall associated twice");
            seen.push(*o);
        }
    }
    assert_eq!(seen.len(), 5);
    // The materials are shared: 5/8" Type X board once.
    assert_eq!(of("IFCMATERIAL").iter().filter(|(_, a)| a.contains("Type X")).count(), 1);
    // Pset_WallCommon per type.
    let psets: Vec<_> = of("IFCPROPERTYSET").into_iter().filter(|(_, a)| a.contains("'Pset_WallCommon'")).collect();
    assert_eq!(psets.len(), 2);
    let props = |p: &str| -> Vec<String> { refs(p).iter().map(|r| all[r].1.clone()).collect() };
    let rated = psets.iter().find(|(_, a)| props(a).iter().any(|v| v.contains("'P1'"))).expect("P1's properties");
    let pv = props(&rated.1);
    assert!(pv.iter().any(|v| v.starts_with("'FireRating',$,IFCLABEL('1 HR')")), "{pv:?}");
    assert!(pv.iter().any(|v| v.starts_with("'AcousticRating',$,IFCLABEL('STC 45')")), "{pv:?}");
    assert!(pv.iter().any(|v| v.starts_with("'IsExternal',$,IFCBOOLEAN(.F.)")), "{pv:?}");
    let ext = psets.iter().find(|(_, a)| props(a).iter().any(|v| v.contains("'X1'"))).expect("X1's properties");
    assert!(props(&ext.1).iter().any(|v| v.starts_with("'IsExternal',$,IFCBOOLEAN(.T.)")));
    let defines = of("IFCRELDEFINESBYPROPERTIES");
    for (pi, _) in &psets {
        assert!(defines.iter().any(|(_, a)| refs(a).last() == Some(pi)), "the property set is assigned");
    }
}

#[test]
fn hostile_assemblies_are_refused_or_survive() {
    let mut s = building(true);
    // A mark on a type that doesn't exist, a wall with one end, a layer 1000" thick.
    for bad in [
        json!({"walls": [{"level": "1", "a": [0, 0], "b": [10, 0], "wall_type": "NOPE"}]}),
        json!({"walls": [{"level": "1", "a": [5, 5], "b": [5, 5], "wall_type": "P1"}]}),
        json!({"wall_types": [{"name": "P1", "assembly": {"layers": [{"material": "x", "thickness_in": 1000}]}}]}),
    ] {
        let r = s.execute("model.build", &json!({ "spec": bad }));
        assert!(r.is_err(), "{bad}");
    }
    // NaN and huge heights in marks never panic the build or the export.
    let r = s
        .execute(
            "bspec.validate",
            &json!({"spec": {"walls": [{"level": "1", "a": [0, 0], "b": [1e300, 1e300], "wall_type": "P1", "height_ft": 1e9}]}}),
        )
        .unwrap();
    assert!(r.to_string().contains("height"), "{r}");
    let _ = cadaddy_io::ifc::write_ifc(s.doc().unwrap(), "x.ifc").unwrap();
}
