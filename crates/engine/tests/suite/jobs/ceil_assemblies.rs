//! Ceiling assemblies, soffits and head of wall framing in the building spec (Q295): ceiling and
//! soffit types, RCP ceilings and soffits naming them, wall types' heads. `model.build` tags the
//! ceilings and soffits with their type (a soffit hung from the structure goes past the ceiling);
//! IFC export writes their IfcMaterialLayerSet (IfcMaterialLayerSetUsage on the third axis) and
//! Pset_CoveringCommon, and each wall type's head; `rcp.takeoff` and `rcp.colour` work from them.

use std::collections::HashMap;

use cadaddy_engine::Session;
use cadaddy_engine::doc::EntityKind;
use serde_json::{Value, json};

fn rect(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("pline", &json!({"vertices": [a, [b[0], a[1]], b, [a[0], b[1]]], "closed": true})).unwrap();
}

fn sq(x0: f64, y0: f64, x1: f64, y1: f64) -> Value {
    json!([[x0, y0], [x1, y0], [x1, y1], [x0, y1]])
}

fn board(m: &str) -> Value {
    json!({"material": m, "thickness_in": 0.625, "category": "board"})
}

/// A made-up 40' × 24' room pair (inches): a toilet room (two-layer gypsum ceiling, a soffit
/// built under it) and an office (tile ceiling, a soffit hung from the structure beside it).
fn building() -> Session {
    let mut s = Session::new();
    if s.execute("layer.new", &json!({"name": "A-WALL", "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": "A-WALL"})).unwrap();
    }
    let t = 7.25;
    rect(&mut s, [0.0, 0.0], [480.0, 288.0]);
    rect(&mut s, [t, t], [480.0 - t, 288.0 - t]);
    let head = json!({"track": "deflection", "to": "deck", "deflection_in": 0.75, "bottom_track": true, "detail": "5"});
    let spec = json!({
        "version": 1,
        "units_per_foot": 12,
        "levels": [{"name": "1", "elevation_ft": 0, "height_ft": 14, "ceiling_ft": 9}],
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "X1"}],
        "wall_types": [{"name": "X1", "thickness_in": 7.25, "from": "pairs", "assembly": {
            "tag": "X1", "height": "full", "external": true, "head": head,
            "layers": [
                {"material": "5/8\" exterior sheathing", "thickness_in": 0.625, "side": "side1", "category": "sheathing"},
                {"material": "6\" metal studs @ 16\" o.c.", "thickness_in": 6.0, "side": "core", "category": "framing", "spacing_in": 16},
                {"material": "5/8\" Type X gypsum board", "thickness_in": 0.625, "side": "side2", "category": "board"}]}}],
        "walls": [
            {"level": "1", "a": [0, 3.625], "b": [480, 3.625], "wall_type": "X1"},
            {"level": "1", "a": [0, 284.375], "b": [480, 284.375], "wall_type": "X1"}
        ],
        "ceiling_types": [
            {"name": "GYP-2", "description": "1 HOUR RATED MOISTURE-RESISTANT GYPSUM BOARD CEILING", "kind": "gypsum", "fire_rating": "1 HR",
             "layers": [board("5/8\" moisture-resistant gypsum board"), board("5/8\" Type X gypsum board"),
                        {"material": "7/8\" furring channel @ 16\" o.c.", "thickness_in": 0.875, "category": "furring", "spacing_in": 16},
                        {"material": "1 1/2\" cold-rolled main runners @ 48\" o.c.", "thickness_in": 1.5, "category": "framing", "spacing_in": 48}]},
            {"name": "ACT-1", "kind": "grid", "grid_in": [24, 48],
             "layers": [{"material": "5/8\" acoustic ceiling tile", "thickness_in": 0.625, "category": "tile"},
                        {"material": "exposed tee grid", "thickness_in": 1.5, "category": "grid"}]}
        ],
        "soffit_types": [
            {"name": "3", "description": "GYP. BD. SOFFIT BUILT UNDER CEILING", "hangs_from": "ceiling",
             "bottom": [board("5/8\" moisture-resistant gypsum board"), board("5/8\" Type X gypsum board")],
             "sides": [board("5/8\" moisture-resistant gypsum board"), board("5/8\" Type X gypsum board")],
             "framing": [{"material": "3 5/8\" 20 ga metal studs @ 16\" o.c.", "thickness_in": 3.625, "category": "framing", "spacing_in": 16}]},
            {"name": "4", "hangs_from": "structure", "above_ceiling_in": 6,
             "bottom": [board("5/8\" gypsum board")], "sides": [board("5/8\" gypsum board")],
             "framing": [{"material": "3 5/8\" 20 ga metal studs @ 16\" o.c.", "thickness_in": 3.625, "category": "framing", "spacing_in": 16}]}
        ],
        "rcp": {
            "ceilings": [
                {"level": "1", "room": "TOILET", "outline": sq(7.25, 7.25, 240.0, 280.75), "height_ft": 8, "ceiling_type": "GYP-2"},
                {"level": "1", "room": "OFFICE", "outline": sq(240.0, 7.25, 472.75, 280.75), "height_ft": 9, "ceiling_type": "ACT-1",
                 "grid": {"spacing_in": [24, 48], "origin": [240, 7.25]}}
            ],
            "soffits": [
                {"level": "1", "outline": sq(7.25, 244.75, 240.0, 280.75), "height_ft": 7, "soffit_type": "3", "top_ft": 8, "sides_ft": 19.396},
                {"level": "1", "outline": sq(240.0, 244.75, 472.75, 280.75), "height_ft": 7.5, "soffit_type": "4", "top_ft": 9, "sides_ft": 19.396}
            ]
        }
    });
    let r = s.execute("bspec.set", &json!({"replace": true, "spec": spec})).unwrap();
    assert!(r["errors"].as_array().is_none_or(|e| e.is_empty()), "{r}");
    s.execute("model.build", &json!({})).unwrap();
    s
}

/// Built ceilings and soffits: (part, type, highest z).
fn coverings(s: &Session) -> Vec<(String, Option<String>, f64)> {
    let mut out = Vec::new();
    for e in s.doc().unwrap().model.iter() {
        let EntityKind::Mesh(m) = &e.kind else { continue };
        let x = &e.common.xdata;
        let Some(at) = x.iter().position(|t| t.code == 1001 && t.value == "CADADDY_WALLGEN") else { continue };
        let tags: Vec<&str> = x.iter().skip(at + 1).take_while(|t| t.code != 1001).map(|t| t.value.as_str()).collect();
        let part = tags.first().copied().unwrap_or_default();
        if part != "ceiling" && part != "soffit" {
            continue;
        }
        let ty = tags.iter().find_map(|v| v.strip_prefix("type:")).map(str::to_string);
        out.push((part.to_string(), ty, m.vertices.iter().map(|v| v.z).fold(f64::MIN, f64::max)));
    }
    out
}

#[test]
fn ceilings_and_soffits_are_built_with_their_types() {
    let s = building();
    let c = coverings(&s);
    let typed = |part: &str, t: &str| c.iter().filter(|(p, x, _)| p == part && x.as_deref() == Some(t)).collect::<Vec<_>>();
    assert_eq!(typed("ceiling", "GYP-2").len(), 1, "{c:?}");
    assert_eq!(typed("ceiling", "ACT-1").len(), 1, "{c:?}");
    // Built under the 8' ceiling: its sides stop there. Hung beside the 9' one: 6" past it.
    let under = typed("soffit", "3");
    let hung = typed("soffit", "4");
    assert!((under[0].2 - 96.0).abs() < 1e-6, "{under:?}");
    assert!((hung[0].2 - 114.0).abs() < 1e-6, "{hung:?}");
}

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
fn ifc_coverings_carry_their_layers_and_walls_their_heads() {
    let s = building();
    let text = String::from_utf8(cadaddy_io::ifc::write_ifc(s.doc().unwrap(), "ceilings.ifc").unwrap()).unwrap();
    let all = instances(&text);
    for (i, (_, a)) in &all {
        for r in refs(a) {
            assert!(all.contains_key(&r), "#{i} refers to #{r}, which isn't there");
        }
    }
    let of = |e: &str| all.iter().filter(|(_, (n, _))| n == e).map(|(i, (_, a))| (*i, a.clone())).collect::<Vec<_>>();
    let coverings: Vec<usize> = of("IFCCOVERING").into_iter().map(|(i, _)| i).collect();
    assert!(coverings.len() >= 4, "{coverings:?}");
    // Two ceiling types and two soffit types, each its own layer set on the third axis.
    let usages: Vec<_> = of("IFCMATERIALLAYERSETUSAGE").into_iter().filter(|(_, u)| u.contains(".AXIS3.")).collect();
    assert_eq!(usages.len(), 4, "{usages:?}");
    let sets: Vec<String> = usages.iter().map(|(_, u)| all[&refs(u)[0]].1.clone()).collect();
    let gyp = sets.iter().find(|a| a.contains("'Ceiling type GYP-2'")).expect("GYP-2's layer set");
    assert_eq!(refs(gyp).len(), 4, "{gyp}");
    let under = sets.iter().find(|a| a.contains("'Soffit type 3'")).expect("soffit 3's layer set");
    assert_eq!(refs(under).len(), 3, "two boards and the studs: {under}");
    for (_, a) in of("IFCRELASSOCIATESMATERIAL") {
        let r = refs(&a);
        let (objs, usage) = r.split_at(r.len() - 1);
        if all[&usage[0]].1.contains(".AXIS3.") {
            assert!(objs.iter().all(|o| coverings.contains(o)));
        }
    }
    let psets = of("IFCPROPERTYSET");
    let props = |p: &str| -> Vec<String> { refs(p).iter().map(|r| all[r].1.clone()).collect() };
    let rated = psets.iter().find(|(_, a)| a.contains("'Pset_CoveringCommon'") && props(a).iter().any(|v| v.contains("'GYP-2'"))).unwrap();
    assert!(props(&rated.1).iter().any(|v| v.starts_with("'FireRating',$,IFCLABEL('1 HR')")));
    let head = psets.iter().find(|(_, a)| a.contains("'CADaddy_HeadOfWall'")).expect("the head of wall properties");
    let hp = props(&head.1);
    assert!(hp.iter().any(|v| v.contains("'slotted deflection track'")), "{hp:?}");
    assert!(hp.iter().any(|v| v.starts_with("'BottomTrack',$,IFCBOOLEAN(.T.)")), "{hp:?}");
}

#[test]
fn takeoff_and_colour_views() {
    let mut s = building();
    let r = s.execute("rcp.takeoff", &json!({})).unwrap();
    let total = |p: &str| r["totals"][p].as_f64().unwrap_or(0.0);
    // The toilet ceiling: 232.75" × 273.5", two layers, the soffit built under it leaves it whole.
    let toilet = 232.75 * 273.5 / 144.0;
    // The office: less the 36" deep soffit hung from the structure.
    let office = 232.75 * (273.5 - 36.0) / 144.0;
    assert!((total("ceiling") - (2.0 * toilet + office)).abs() < 1e-6, "{r}");
    let soffit = 232.75 * 36.0 / 144.0;
    assert!((total("soffit bottom") - 3.0 * soffit).abs() < 1e-6, "{r}");
    // Sides: 19.396' × 1' × 2 layers, and × 2' (1'-6" plus 6" past the ceiling) × 1 layer.
    assert!((total("soffit sides") - (19.396 * 2.0 + 19.396 * 2.0)).abs() < 1e-6, "{r}");
    assert!((total("top track") - 80.0).abs() < 1e-6 && (total("bottom track") - 80.0).abs() < 1e-6, "{r}");
    assert!(r["csv"].as_str().unwrap().starts_with("level,part,assembly,material,layer,quantity,unit\n"));
    assert!(r["rows"].as_array().unwrap().iter().any(|x| x["layer"] == 2 && x["assembly"] == "GYP-2"));

    let c = s.execute("rcp.colour", &json!({"by": "soffit"})).unwrap();
    let keys: Vec<&str> = c["legend"].as_array().unwrap().iter().filter_map(|x| x["key"].as_str()).collect();
    assert_eq!(keys, ["soffit built under the ceiling", "soffit hung from the structure"], "{c}");
    let c = s.execute("rcp.colour", &json!({"by": "layers"})).unwrap();
    let keys: Vec<&str> = c["legend"].as_array().unwrap().iter().filter_map(|x| x["key"].as_str()).collect();
    assert!(keys.contains(&"2 board layers") && keys.contains(&"1 board layer"), "{keys:?}");
    let c = s.execute("rcp.colour", &json!({"by": "head"})).unwrap();
    assert!(c["legend"][0]["key"].as_str().unwrap().contains("deflection"), "{c}");
    let coloured = |s: &Session| s.doc().unwrap().model.iter().filter(|e| e.common.color != cadaddy_engine::doc::color::Color::ByLayer).count();
    assert!(coloured(&s) > 0);
    s.execute("rcp.colour", &json!({"by": "none"})).unwrap();
    assert_eq!(coloured(&s), 0);
    assert!(s.execute("rcp.colour", &json!({"by": "nonsense"})).is_err());
}

#[test]
fn hostile_ceiling_types_are_refused_or_survive() {
    let mut s = building();
    for bad in [
        json!({"rcp": {"ceilings": [{"level": "1", "outline": sq(0.0, 0.0, 10.0, 10.0), "height_ft": 8, "ceiling_type": "NOPE"}]}}),
        json!({"ceiling_types": [{"name": "GYP-2", "layers": [{"material": "x", "thickness_in": 1000}]}]}),
        json!({"soffit_types": [{"name": "3", "framing": [{"material": "x", "thickness_in": 1, "spacing_in": 0}]}]}),
        json!({"rcp": {"soffits": [{"level": "1", "outline": sq(0.0, 0.0, 10.0, 10.0), "height_ft": 8, "top_ft": 2, "soffit_type": "3"}]}}),
    ] {
        assert!(s.execute("model.build", &json!({ "spec": bad })).is_err(), "{bad}");
    }
    let r = s
        .execute(
            "bspec.validate",
            &json!({"spec": {"wall_types": [{"name": "X1", "assembly": {"head": {"track": "double", "to": "deck", "deflection_in": 1e9}}}]}}),
        )
        .unwrap();
    assert!(r.to_string().contains("deflection"), "{r}");
    for p in [json!({"level": 5}), json!({"level": "\u{0}"}), json!(null), json!({"by": 3})] {
        let _ = s.execute("rcp.takeoff", &p);
        let _ = s.execute("rcp.colour", &p);
    }
    let _ = cadaddy_io::ifc::write_ifc(s.doc().unwrap(), "x.ifc").unwrap();
}

/// The WALLGEN walls' handles.
fn walls(s: &Session) -> Vec<String> {
    s.doc().unwrap().model.iter().filter(|e| cadaddy_engine::cmd::wallgen::made_as(e).as_deref() == Some("wall")).map(|e| e.handle.hex()).collect()
}

#[test]
fn bom_carries_ceilings_soffits_and_head_track_once() {
    let mut s = building();
    let toilet = 232.75 * 273.5 / 144.0;
    let office = 232.75 * (273.5 - 36.0) / 144.0;
    let soffit = 232.75 * 36.0 / 144.0;
    let b = s.execute("bim.bom", &json!({})).unwrap();
    let lines: Vec<&Value> = b["lines"].as_array().unwrap().iter().filter(|l| l["trade"] == "drywall").collect();
    let sum = |f: &dyn Fn(&str) -> bool| lines.iter().filter(|l| f(l["item"].as_str().unwrap())).map(|l| l["net"].as_f64().unwrap()).sum::<f64>();
    // Each takeoff row and BOM line is rounded to the cent.
    let near = |a: f64, b: f64| (a - b).abs() < 0.05;
    let ceiling = sum(&|i| i.starts_with("Ceiling board "));
    assert!(near(ceiling, 2.0 * toilet + office), "{ceiling} {b:#}");
    // By type and layer: GYP-2 has two lines, one a layer.
    let mut gyp: Vec<&str> =
        lines.iter().filter(|l| l["size"].as_str().unwrap().starts_with("type GYP-2, layer")).map(|l| l["size"].as_str().unwrap()).collect();
    gyp.sort_unstable();
    assert_eq!(gyp, ["type GYP-2, layer 1", "type GYP-2, layer 2"], "{b:#}");
    let bottom = sum(&|i| i.starts_with("Soffit bottom board "));
    assert!(near(bottom, 3.0 * soffit), "{bottom} {b:#}");
    assert!(near(sum(&|i| i.starts_with("Soffit side board ")), 19.396 * 2.0 + 19.396 * 2.0), "{b:#}");
    assert!(sum(&|i| i.starts_with("Ceiling framing ")) > 0.0 && sum(&|i| i.starts_with("Soffit framing ")) > 0.0, "{b:#}");
    // The two marked 40' walls take slotted deflection track and bottom track from their head of wall.
    assert_eq!(sum(&|i| i == "Slotted deflection track"), 80.0, "{b:#}");
    assert_eq!(sum(&|i| i == "Bottom track"), 80.0, "{b:#}");
    // Every wall's track once: 40' + 40' + 24' + 24' walls, a top and a bottom run each; the unmarked
    // 24' walls keep the plain track.
    assert_eq!(sum(&|i| i == "Track"), 2.0 * 48.0, "{b:#}");
    let track = sum(&|i| i.ends_with("track") || i == "Track");
    assert_eq!(track, 2.0 * (40.0 + 40.0 + 24.0 + 24.0), "{b:#}");

    // drywall.takeoff carries the same: head track by kind in the rows, ceilings beside them.
    let t = s.execute("drywall.takeoff", &json!({"waste": 0})).unwrap();
    assert_eq!(t["totals"]["trackLengthFt"], 256.0, "{t:#}");
    assert_eq!(t["totals"]["headTrackLengthFt"], 80.0, "{t:#}");
    assert_eq!(t["totals"]["bottomTrackLengthFt"], 80.0, "{t:#}");
    assert!(near(t["totals"]["ceilingBoardAreaSqFt"].as_f64().unwrap(), 2.0 * toilet + office), "{t:#}");
    let head = t["rows"][0]["headTrack"].as_array().unwrap();
    assert!(head.iter().any(|h| h["material"] == "slotted deflection track" && h["lengthFt"] == 80.0 && h["wallType"] == "X1"), "{t:#}");
    assert!(t["ceilings"].as_array().unwrap().iter().all(|c| c["part"] != "top track" && c["part"] != "bottom track"));
    // A wall-type filter leaves the ceilings out; a level filter keeps them.
    let t = s.execute("drywall.takeoff", &json!({"wallTypes": ["X1"]})).unwrap();
    assert_eq!(t["ceilings"].as_array().unwrap().len(), 0, "{t:#}");
    let t = s.execute("drywall.takeoff", &json!({"levels": ["1"]})).unwrap();
    assert!(!t["ceilings"].as_array().unwrap().is_empty(), "{t:#}");
    let t = s.execute("drywall.takeoff", &json!({"levels": ["2"]})).unwrap();
    assert_eq!(t["ceilings"].as_array().unwrap().len(), 0, "{t:#}");

    // The CSV has them too.
    let dir = std::env::temp_dir().join(format!("ceil-takeoff-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Sample Takeoff.csv");
    s.execute("drywall.takeoff", &json!({"file": path.to_str().unwrap()})).unwrap();
    let csv = std::fs::read_to_string(&path).unwrap();
    assert!(csv.lines().any(|l| l.contains("slotted deflection track") && l.contains(",80.00,")), "{csv}");
    assert!(csv.lines().any(|l| l.starts_with("ceiling GYP-2 (1),")), "{csv}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn wall_info_shows_the_head_of_wall() {
    let mut s = building();
    let i = s.execute("wall.info", &json!({"handles": walls(&s)})).unwrap();
    let ws = i["walls"].as_array().unwrap();
    assert_eq!(ws.len(), 4, "{i:#}");
    for w in ws {
        assert_eq!(w["wallType"], "X1");
        let h = &w["assembly"]["head"];
        assert_eq!(h["track"], "slotted deflection track", "{i:#}");
        assert_eq!(h["trackKind"], "deflection");
        assert_eq!(h["to"], "deck");
        assert_eq!(h["deflectionIn"], 0.75);
        assert_eq!(h["bottomTrack"], true);
        assert_eq!(h["detail"], "5");
        assert!(h["text"].as_str().unwrap().contains("slotted deflection track at the deck"), "{h}");
    }
}
