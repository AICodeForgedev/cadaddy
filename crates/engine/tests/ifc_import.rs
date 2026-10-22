//! IFC models as coordination references (`ifc.attach`): our own IFC export attached back gives
//! the same objects, classes, storeys and GlobalIds in the same places; an architecture model
//! written by hand (metres) clashes with the MEP runs, keeps its properties, storeys and systems,
//! and reloads when its file changes.

use std::collections::{BTreeMap, BTreeSet};

use cadaddy_engine::Session;
use cadaddy_engine::doc::{Drawing, Entity, EntityKind, Handle, Line, Mesh};
use cadaddy_engine::geom::Vec3;
use serde_json::{Value, json};

fn hooks() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None, read_dwf: None, write_dwfx: None,
    });
}

fn dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-ifcimport-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Feet → a line in inches on A-WALL.
fn line(d: &mut Drawing, a: (f64, f64), b: (f64, f64)) {
    let p = |(x, y): (f64, f64)| Vec3 { x: x * 12.0, y: y * 12.0, z: 0.0 };
    let h = Handle(d.handseed.max(0x100));
    d.handseed = h.0 + 1;
    let mut e = Entity::new(h, EntityKind::Line(Line { a: p(a), b: p(b) }));
    e.common.layer = "A-WALL".into();
    d.model.push(e);
}

/// A two-room plan with a doorway, built by WALLGEN (walls, slab, ceilings, door).
fn building() -> Session {
    let mut d = Drawing::new_imperial();
    for (a, b) in [((0., 0.), (40., 0.)), ((40., 0.), (40., 10.)), ((40., 10.), (0., 10.)), ((0., 10.), (0., 0.))] {
        line(&mut d, a, b);
    }
    for (a, b) in [((0.5, 0.5), (39.5, 0.5)), ((39.5, 0.5), (39.5, 9.5)), ((39.5, 9.5), (0.5, 9.5)), ((0.5, 9.5), (0.5, 0.5))] {
        line(&mut d, a, b);
    }
    for x in [20.0, 20.5] {
        line(&mut d, (x, 0.5), (x, 3.0));
        line(&mut d, (x, 6.0), (x, 9.5));
    }
    let mut s = Session::empty();
    s.open_drawing(d, "Sample Plan", None);
    let rules = json!({
        "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 9.5, "ceiling_ft": 8, "base": [0, 0]}],
        "layers": [{"layer": "A-WALL", "kind": "wall", "skip_curves": true}]
    });
    s.execute("wallgen", &json!({ "rules": rules })).unwrap();
    s
}

/// The GlobalIds of the products (objects with a body) in IFC text, with their entity names.
fn products(ifc: &str) -> BTreeMap<String, String> {
    let shapes: BTreeSet<String> =
        ifc.lines().filter(|l| l.contains("=IFCPRODUCTDEFINITIONSHAPE(")).filter_map(|l| l.split('=').next().map(str::to_string)).collect();
    let mut out = BTreeMap::new();
    for l in ifc.lines() {
        let Some((_, rest)) = l.split_once('=') else { continue };
        if !shapes.iter().any(|s| rest.contains(&format!(",{s},"))) {
            continue;
        }
        let entity = rest.split('(').next().unwrap_or_default().to_string();
        let guid = rest.split('\'').nth(1).unwrap_or_default().to_string();
        out.insert(guid, entity);
    }
    out
}

fn bounds(pts: impl Iterator<Item = Vec3>) -> (Vec3, Vec3) {
    pts.fold((Vec3::new(f64::MAX, f64::MAX, f64::MAX), Vec3::new(f64::MIN, f64::MIN, f64::MIN)), |(lo, hi), v| {
        (Vec3::new(lo.x.min(v.x), lo.y.min(v.y), lo.z.min(v.z)), Vec3::new(hi.x.max(v.x), hi.y.max(v.y), hi.z.max(v.z)))
    })
}

fn v3(v: &Value) -> Vec3 {
    Vec3::new(v[0].as_f64().unwrap(), v[1].as_f64().unwrap(), v[2].as_f64().unwrap())
}

#[test]
fn our_own_export_attaches_back_with_the_same_objects_and_globalids() {
    hooks();
    let tmp = dir("roundtrip");
    let file = tmp.join("Sample Plan.ifc");
    let mut s = building();
    let meshes: Vec<Mesh> =
        s.doc().unwrap().model.iter().filter_map(|e| if let EntityKind::Mesh(m) = &e.kind { Some(m.clone()) } else { None }).collect();
    assert!(meshes.len() > 5, "the building has walls, a slab, ceilings, a door");
    let (lo0, hi0) = bounds(meshes.iter().flat_map(|m| m.vertices.iter().copied()));
    s.execute("export", &json!({"path": file.to_string_lossy()})).unwrap();
    let text = std::fs::read_to_string(&file).unwrap();
    let written = products(&text);
    assert_eq!(written.len(), meshes.len(), "one product per mesh: {written:?}");

    let r = s.execute("ifc.attach", &json!({"path": file.to_string_lossy()})).unwrap();
    assert_eq!(r["name"], json!("Sample Plan"));
    assert_eq!(r["elements"], json!(meshes.len()), "{r}");
    // An inch drawing: metres come in at 39.37 units each.
    assert!((r["scale"].as_f64().unwrap() - 1.0 / 0.0254).abs() < 1e-6, "{r}");
    // One storey, at the spec's level.
    assert_eq!(r["levels"], json!([{"name": "L1", "elevation": 0.0, "elements": meshes.len(), "spec_level": "L1"}]), "{r}");

    let els = s.execute("ifc.elements", &json!({"limit": 1000})).unwrap();
    assert_eq!(els["total"], json!(meshes.len()));
    let list = els["elements"].as_array().unwrap();
    // The same GlobalIds, each as the class it was written as.
    let read: BTreeMap<String, String> =
        list.iter().map(|e| (e["guid"].as_str().unwrap().to_string(), format!("IFC{}", e["class"].as_str().unwrap().to_ascii_uppercase()))).collect();
    assert_eq!(read, written);
    // In the same place: the reference's objects span the building's meshes exactly.
    let (lo, hi) = bounds(list.iter().flat_map(|e| [v3(&e["min"]), v3(&e["max"])]));
    for (a, b) in [(lo.x, lo0.x), (lo.y, lo0.y), (lo.z, lo0.z), (hi.x, hi0.x), (hi.y, hi0.y), (hi.z, hi0.z)] {
        assert!((a - b).abs() < 1e-6, "{lo:?} {hi:?} vs {lo0:?} {hi0:?}");
    }
    // Kept apart: on its own layer, the objects on the reference's class layers.
    let d = s.doc().unwrap();
    let ins = d.model.iter().find(|e| matches!(&e.kind, EntityKind::Insert(i) if i.block == "Sample Plan")).unwrap();
    assert_eq!(ins.common.layer, "X-IFC-Sample Plan");
    assert!(d.layer("Sample Plan|IFC-Wall").is_some());
    let refs = s.execute("ifc.refs", &json!({})).unwrap();
    assert_eq!((refs["refs"][0]["status"].clone(), refs["refs"][0]["changed"].clone()), (json!("Loaded"), json!(false)), "{refs}");

    // And the other way: the file opened as a drawing and exported again keeps every GlobalId.
    let opened = cadaddy_io::read(&std::fs::read(&file).unwrap(), "Sample Plan.ifc").unwrap();
    let again = String::from_utf8(cadaddy_io::write(&opened, "Again.ifc").unwrap()).unwrap();
    assert_eq!(products(&again).keys().collect::<Vec<_>>(), written.keys().collect::<Vec<_>>());

    // Undo takes the reference away.
    s.execute("undo", &json!({})).unwrap();
    assert!(s.execute("ifc.refs", &json!({})).unwrap()["refs"].as_array().unwrap().is_empty());
    let _ = std::fs::remove_dir_all(&tmp);
}

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

fn text(s: &mut Session, at: [f64; 2], t: &str) {
    s.execute("text", &json!({"at": at, "text": t, "height": 4})).unwrap();
}

/// The MEP runs of the 3D MEP tests (inches): a 24x12 supply duct at 10' along y = 60 from x = 24
/// to 264 (dropping to 8' round the corner), cold water across x = 100 at 10'-6", a conduit high
/// up along y = 300.
fn mep() -> Session {
    let mut s = Session::new();
    layer(&mut s, "M-DUCT-SUPP");
    s.execute("pline", &json!({"vertices": [[24, 60], [264, 60], [264, 180]]})).unwrap();
    text(&mut s, [180.0, 70.0], "24x12 BOT @ 10'-0\"");
    text(&mut s, [280.0, 140.0], "BOT @ 8'-0\"");
    layer(&mut s, "P-DOMW-CW");
    s.execute("line", &json!({"points": [[100, 0], [100, 120]]})).unwrap();
    text(&mut s, [104.0, 20.0], "2\" CW CL @ 10'-6\"");
    layer(&mut s, "E-COND");
    s.execute("line", &json!({"points": [[300, 300], [400, 300]]})).unwrap();
    text(&mut s, [340.0, 306.0], "1\" C BOT @ 13'-10\"");
    s.execute(
        "bspec.set",
        &json!({"replace": true, "spec": {"levels": [
            {"name": "L1", "elevation_ft": 0, "height_ft": 14, "ceiling_ft": 9, "region": [-100, -100, 1000, 1000], "base": [0, 0]}
        ]}}),
    )
    .unwrap();
    s.execute("mep.read", &json!({"apply": true})).unwrap();
    s
}

/// An architecture model in metres from "another program": a 200 mm partition crossing the duct
/// at `wall_x` metres (y 0–3 m, 4 m high) with its fire rating and type, a slab over everything
/// at 4.0–4.2 m, a room volume round everything, and a supply system on the slab.
fn architecture(wall_x: f64) -> String {
    format!(
        "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('ViewDefinition [CoordinationView]'),'2;1');\nFILE_SCHEMA(('IFC2X3'));\nENDSEC;\nDATA;\n\
#1=IFCSIUNIT(*,.LENGTHUNIT.,$,.METRE.);\n#2=IFCUNITASSIGNMENT((#1));\n\
#3=IFCPROJECT('0Prj000000000000000001',$,'Sample Project',$,$,$,$,$,#2);\n\
#4=IFCBUILDING('0Bld000000000000000001',$,'Building A',$,$,$,$,$,.ELEMENT.,$,$,$);\n\
#5=IFCBUILDINGSTOREY('0Sty000000000000000001',$,'Ground Floor',$,$,$,$,$,.ELEMENT.,0.);\n\
#6=IFCRELAGGREGATES('0Rel000000000000000001',$,$,$,#4,(#5));\n\
#7=IFCDIRECTION((0.,0.,1.));\n\
#10=IFCCARTESIANPOINT(({wall_x},1.5));\n#11=IFCAXIS2PLACEMENT2D(#10,$);\n#12=IFCRECTANGLEPROFILEDEF(.AREA.,$,#11,0.2,3.);\n\
#13=IFCEXTRUDEDAREASOLID(#12,$,#7,4.);\n#14=IFCSHAPEREPRESENTATION($,'Body','SweptSolid',(#13));\n#15=IFCPRODUCTDEFINITIONSHAPE($,$,(#14));\n\
#16=IFCWALLSTANDARDCASE('2Wal000000000000000001',$,'Partition P1',$,$,$,#15,$);\n\
#20=IFCCARTESIANPOINT((5.,5.));\n#21=IFCAXIS2PLACEMENT2D(#20,$);\n#22=IFCRECTANGLEPROFILEDEF(.AREA.,$,#21,10.,10.);\n\
#23=IFCCARTESIANPOINT((0.,0.,4.));\n#24=IFCAXIS2PLACEMENT3D(#23,$,$);\n\
#25=IFCEXTRUDEDAREASOLID(#22,#24,#7,0.2);\n#26=IFCSHAPEREPRESENTATION($,'Body','SweptSolid',(#25));\n#27=IFCPRODUCTDEFINITIONSHAPE($,$,(#26));\n\
#28=IFCSLAB('2Slb000000000000000001',$,'Floor Above',$,$,$,#27,$,.FLOOR.);\n\
#30=IFCEXTRUDEDAREASOLID(#22,$,#7,4.);\n#31=IFCSHAPEREPRESENTATION($,'Body','SweptSolid',(#30));\n#32=IFCPRODUCTDEFINITIONSHAPE($,$,(#31));\n\
#33=IFCSPACE('2Spc000000000000000001',$,'101',$,$,$,#32,'Open Office',.ELEMENT.,.INTERNAL.,$);\n\
#40=IFCRELCONTAINEDINSPATIALSTRUCTURE('0Rel000000000000000002',$,$,$,(#16,#28),#5);\n\
#41=IFCRELAGGREGATES('0Rel000000000000000003',$,$,$,#5,(#33));\n\
#42=IFCPROPERTYSINGLEVALUE('FireRating',$,IFCLABEL('2HR'),$);\n#43=IFCPROPERTYSET('0Pst000000000000000001',$,'Pset_WallCommon',$,(#42));\n\
#44=IFCRELDEFINESBYPROPERTIES('0Rel000000000000000004',$,$,$,(#16),#43);\n\
#45=IFCWALLTYPE('0Typ000000000000000001',$,'Partition 200',$,$,$,$,$,$,.STANDARD.);\n\
#46=IFCRELDEFINESBYTYPE('0Rel000000000000000005',$,$,$,(#16),#45);\n\
#47=IFCSYSTEM('0Sys000000000000000001',$,'Structure',$,$);\n#48=IFCRELASSIGNSTOGROUP('0Rel000000000000000006',$,$,$,(#28),$,#47);\n\
ENDSEC;\nEND-ISO-10303-21;\n"
    )
}

fn reference_clashes(s: &mut Session, p: Value) -> Vec<Value> {
    let c = s.execute("model.clashes", &p).unwrap();
    c["clashes"].as_array().unwrap().iter().filter(|x| x["kind"] == "reference").cloned().collect()
}

#[test]
fn mep_runs_clash_with_an_architecture_model_that_reloads_when_its_file_changes() {
    hooks();
    let tmp = dir("clash");
    let file = tmp.join("Architecture.ifc");
    // The wall at x = 150" (3.81 m) crosses the duct's first leg.
    std::fs::write(&file, architecture(3.81)).unwrap();
    let mut s = mep();
    let before = s.execute("model.clashes", &json!({})).unwrap()["total"].clone();
    let r = s.execute("ifc.attach", &json!({"path": file.to_string_lossy(), "name": "Arch"})).unwrap();
    assert_eq!(r["classes"], json!({"Slab": 1, "Space": 1, "WallStandardCase": 1}), "{r}");
    // The spec's units: 12 per foot.
    assert!((r["scale"].as_f64().unwrap() - 12.0 / 0.3048).abs() < 1e-6);
    // The storey matches level L1 by its elevation.
    assert_eq!(r["levels"][0]["spec_level"], json!("L1"), "{r}");

    // Properties, type, storey and system of each object.
    let w = s.execute("ifc.elements", &json!({"class": "IfcWall"})).unwrap();
    assert_eq!(w["total"], json!(1), "{w}");
    let wall = &w["elements"][0];
    assert_eq!(
        (wall["guid"].clone(), wall["name"].clone(), wall["type"].clone(), wall["level"].clone()),
        (json!("2Wal000000000000000001"), json!("Partition P1"), json!("Partition 200"), json!("Ground Floor"))
    );
    assert_eq!(wall["properties"]["Pset_WallCommon.FireRating"], json!("2HR"));
    // 150" ± 3.94" (100 mm), 4 m = 157.48" high.
    assert!(
        (wall["min"][0].as_f64().unwrap() - (3.71 / 0.0254)).abs() < 1e-6 && (wall["max"][2].as_f64().unwrap() - 4.0 / 0.0254).abs() < 1e-6,
        "{wall}"
    );
    let by_prop = s.execute("ifc.elements", &json!({"property": "Pset_WallCommon.FireRating=2hr"})).unwrap();
    assert_eq!(by_prop["total"], json!(1));
    assert_eq!(s.execute("ifc.elements", &json!({"property": "Pset_WallCommon.FireRating=1HR"})).unwrap()["total"], json!(0));
    assert_eq!(s.execute("ifc.elements", &json!({"system": "structure"})).unwrap()["elements"][0]["class"], json!("Slab"));
    assert_eq!(s.execute("ifc.elements", &json!({"level": "ground floor"})).unwrap()["total"], json!(3));
    assert!(s.execute("ifc.elements", &json!({"name": "Nothing"})).is_err());

    // The duct runs into the wall; the room volume and the slab above (4 m, over the 11' duct
    // top) don't count. Nothing else changes.
    let c = reference_clashes(&mut s, json!({}));
    assert_eq!(c.len(), 1, "{c:?}");
    assert_eq!(c[0]["systems"], json!(["M-DUCT-SUPP"]));
    assert_eq!(c[0]["reference"]["guid"], json!("2Wal000000000000000001"));
    assert_eq!(c[0]["reference"]["class"], json!("WallStandardCase"));
    let at = &c[0]["at"];
    assert!((at[0].as_f64().unwrap() - 150.0).abs() < 4.0 && (at[1].as_f64().unwrap() - 60.0).abs() < 12.5, "{at}");
    let total = s.execute("model.clashes", &json!({"references": false})).unwrap()["total"].clone();
    assert_eq!(total, before);
    // Asked to, room volumes clash too: the duct and the pipe are inside the room.
    let all = reference_clashes(&mut s, json!({"ignore_classes": []}));
    let pairs: BTreeSet<(String, String)> =
        all.iter().map(|x| (x["systems"][0].as_str().unwrap().to_string(), x["reference"]["class"].as_str().unwrap().to_string())).collect();
    for p in [("M-DUCT-SUPP", "Space"), ("P-DOMW-CW", "Space"), ("M-DUCT-SUPP", "WallStandardCase")] {
        assert!(pairs.contains(&(p.0.to_string(), p.1.to_string())), "{p:?} in {pairs:?}");
    }
    assert!(!pairs.iter().any(|(sys, _)| sys == "E-COND"), "the conduit is above the room: {pairs:?}");
    // A tolerance wider than the wall's overlap forgives it.
    assert!(reference_clashes(&mut s, json!({"tolerance_in": 13})).is_empty());

    // The other program moves the wall clear of the duct (x = 8 m, past its end at 264").
    assert_eq!(s.execute("ifc.reload", &json!({"changed_only": true})).unwrap()["unchanged"], json!(["Arch"]));
    std::fs::write(&file, architecture(8.0) + "\n").unwrap();
    assert_eq!(s.execute("ifc.refs", &json!({})).unwrap()["refs"][0]["changed"], json!(true));
    let r = s.execute("ifc.reload", &json!({"changed_only": true})).unwrap();
    assert_eq!(r["reloaded"], json!(["Arch"]), "{r}");
    assert!(reference_clashes(&mut s, json!({})).is_empty());
    assert_eq!(s.execute("ifc.refs", &json!({})).unwrap()["refs"][0]["changed"], json!(false));

    // The app's check finds the next change by itself, and leaves an unchanged file alone.
    assert!(cadaddy_engine::cmd::ifcref::poll(&mut s).is_empty());
    std::fs::write(&file, architecture(3.81) + "\n\n").unwrap();
    assert_eq!(cadaddy_engine::cmd::ifcref::poll(&mut s), vec!["Arch".to_string()]);
    assert_eq!(reference_clashes(&mut s, json!({})).len(), 1);

    // Saved with the drawing as a reference (not its objects), loaded again when it opens, in
    // .cdy and in DXF.
    for ext in ["cdy", "dxf"] {
        let saved = tmp.join(format!("Coordination.{ext}"));
        s.execute("saveas", &json!({"path": saved.to_string_lossy()})).unwrap();
        let mut t = Session::new();
        t.execute("open", &json!({"path": saved.to_string_lossy()})).unwrap();
        let refs = t.execute("ifc.refs", &json!({})).unwrap();
        assert_eq!(
            (refs["refs"][0]["name"].clone(), refs["refs"][0]["status"].clone(), refs["refs"][0]["elements"].clone()),
            (json!("Arch"), json!("Loaded"), json!(3)),
            "{ext}: {refs}"
        );
        assert_eq!(reference_clashes(&mut t, json!({})).len(), 1, "{ext}");
    }

    // A file that goes bad on disk: the reload says why, nothing panics.
    std::fs::write(&file, "ISO-10303-21;\nDATA;\n#1=IFCWALL(((((\n").unwrap();
    let r = s.execute("ifc.reload", &json!({})).unwrap();
    assert_eq!(r["failed"][0]["name"], json!("Arch"), "{r}");
    assert!(reference_clashes(&mut s, json!({})).is_empty());
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn bad_attachments_are_refused_without_panicking() {
    hooks();
    let tmp = dir("bad");
    let mut s = mep();
    assert!(s.execute("ifc.attach", &json!({})).is_err());
    assert!(s.execute("ifc.attach", &json!({"path": "plan.dwg"})).is_err());
    assert!(s.execute("ifc.attach", &json!({"path": tmp.join("Missing.ifc").to_string_lossy()})).is_err());
    let junk = tmp.join("Junk.ifc");
    std::fs::write(&junk, b"not a model at all").unwrap();
    assert!(s.execute("ifc.attach", &json!({"path": junk.to_string_lossy()})).is_err());
    let good = tmp.join("Arch.ifc");
    std::fs::write(&good, architecture(3.81)).unwrap();
    for p in [json!({"at": "here"}), json!({"at": [1]}), json!({"scale": 0}), json!({"scale": -3}), json!({"scale": "big"})] {
        let mut q = p.clone();
        q["path"] = json!(good.to_string_lossy());
        assert!(s.execute("ifc.attach", &q).is_err(), "{p}");
    }
    // A failed attach leaves nothing behind; a good one at a point, turned, in 3D.
    assert!(s.execute("ifc.refs", &json!({})).unwrap()["refs"].as_array().unwrap().is_empty());
    let r = s.execute("ifc.attach", &json!({"path": good.to_string_lossy(), "at": [1000, 0, 24], "rotation": 90})).unwrap();
    let wall = s.execute("ifc.elements", &json!({"class": "wall"})).unwrap()["elements"][0].clone();
    // Turned 90°: x = 1000 - y; raised 24.
    assert!((wall["max"][0].as_f64().unwrap() - 1000.0).abs() < 1e-6 && (wall["min"][2].as_f64().unwrap() - 24.0).abs() < 1e-6, "{wall}");
    assert_eq!(r["name"], json!("Arch"));
    // Hostile filters and limits.
    for p in [json!({"limit": -1}), json!({"limit": u64::MAX}), json!({"class": 7}), json!({"property": "=="}), json!({"guid": ""})] {
        assert!(s.execute("ifc.elements", &p).is_ok(), "{p}");
    }
    assert!(s.execute("model.clashes", &json!({"ignore_classes": "Space", "tolerance_in": -4})).is_ok());
    assert!(s.execute("ifc.reload", &json!({"names": 5})).is_ok());
    let _ = std::fs::remove_dir_all(&tmp);
}
