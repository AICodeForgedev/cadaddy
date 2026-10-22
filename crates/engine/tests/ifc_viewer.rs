//! The IFC model viewer and BCF issues (Q283): a made-up IFC4 model (written here, metres) opened
//! as a model — its spatial tree, one object's property and quantity sets, filters that isolate
//! and hide, the section box, measuring, a render, Walk's scene — and BCF topics raised on it,
//! passed to another session through a `.bcf` file and back, read from a file another program
//! might write, kept in the drawing when saved, and refused safely when the file is junk.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::ifcmodel;
use serde_json::{Value, json};

fn hooks() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
    });
}

fn dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-ifcviewer-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// STEP text, numbered as it is written.
#[derive(Default)]
struct Step {
    lines: Vec<String>,
}

impl Step {
    fn add(&mut self, body: impl Into<String>) -> usize {
        self.lines.push(body.into());
        self.lines.len()
    }

    /// A box `size` (metres) with its base centred at `at`: (placement, product shape).
    fn boxed(&mut self, at: [f64; 3], size: [f64; 3]) -> (usize, usize) {
        let p = self.add(format!("IFCCARTESIANPOINT(({:.3},{:.3},{:.3}))", at[0], at[1], at[2]));
        let a = self.add(format!("IFCAXIS2PLACEMENT3D(#{p},$,$)"));
        let place = self.add(format!("IFCLOCALPLACEMENT($,#{a})"));
        let c = self.add("IFCCARTESIANPOINT((0.,0.))");
        let a2 = self.add(format!("IFCAXIS2PLACEMENT2D(#{c},$)"));
        let r = self.add(format!("IFCRECTANGLEPROFILEDEF(.AREA.,$,#{a2},{:.3},{:.3})", size[0], size[1]));
        let d = self.add("IFCDIRECTION((0.,0.,1.))");
        let e = self.add(format!("IFCEXTRUDEDAREASOLID(#{r},$,#{d},{:.3})", size[2]));
        let s = self.add(format!("IFCSHAPEREPRESENTATION($,'Body','SweptSolid',(#{e}))"));
        let pds = self.add(format!("IFCPRODUCTDEFINITIONSHAPE($,$,(#{s}))"));
        (place, pds)
    }

    fn text(&self) -> String {
        let mut t = String::from(
            "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION((''),'2;1');\nFILE_NAME('viewer.ifc','2026-10-21T00:00:00',(''),(''),'','','');\nFILE_SCHEMA(('IFC4'));\nENDSEC;\nDATA;\n",
        );
        for (i, l) in self.lines.iter().enumerate() {
            t += &format!("#{}={l};\n", i + 1);
        }
        t + "ENDSEC;\nEND-ISO-10303-21;\n"
    }
}

const WALL_N: &str = "1WalNorth0000000000001";
const WALL_W: &str = "1WalWest00000000000001";
const WALL_2: &str = "1WalUpper0000000000001";
const COLUMN: &str = "1Col00000000000000001A";
const DUCT: &str = "1Dct00000000000000001A";
const DOOR: &str = "1Dor00000000000000001A";

/// A two-storey corner of a made-up hotel: site "Site A", building "Hotel B", Level 1 (0 m) with
/// two walls, a door, a column, a slab and room 101 (a duct in it, in the Supply Air system),
/// Level 2 (3.5 m) with a wall and a slab.
fn hotel_ifc() -> String {
    let mut s = Step::default();
    let unit = s.add("IFCSIUNIT(*,.LENGTHUNIT.,$,.METRE.)");
    let units = s.add(format!("IFCUNITASSIGNMENT((#{unit}))"));
    let project = s.add(format!("IFCPROJECT('0Prj000000000000000001',$,'Hotel',$,$,$,$,$,#{units})"));
    let site = s.add("IFCSITE('0Sit000000000000000001',$,'Site A',$,$,$,$,$,.ELEMENT.,$,$,$,$,$)");
    let bldg = s.add("IFCBUILDING('0Bld000000000000000001',$,'Hotel B',$,$,$,$,$,.ELEMENT.,$,$,$)");
    let l1 = s.add("IFCBUILDINGSTOREY('0Sty000000000000000001',$,'Level 1',$,$,$,$,$,.ELEMENT.,0.)");
    let l2 = s.add("IFCBUILDINGSTOREY('0Sty000000000000000002',$,'Level 2',$,$,$,$,$,.ELEMENT.,3.5)");
    s.add(format!("IFCRELAGGREGATES('0Rel000000000000000001',$,$,$,#{project},(#{site}))"));
    s.add(format!("IFCRELAGGREGATES('0Rel000000000000000002',$,$,$,#{site},(#{bldg}))"));
    s.add(format!("IFCRELAGGREGATES('0Rel000000000000000003',$,$,$,#{bldg},(#{l1},#{l2}))"));
    let product = |s: &mut Step, entity: &str, guid: &str, name: &str, at: [f64; 3], size: [f64; 3], tail: &str| {
        let (place, pds) = s.boxed(at, size);
        s.add(format!("{entity}('{guid}',$,'{name}',$,$,#{place},#{pds},$,{tail})"))
    };
    let wn = product(&mut s, "IFCWALL", WALL_N, "Wall North", [5.0, 0.0, 0.0], [10.0, 0.2, 3.0], ".STANDARD.");
    let ww = product(&mut s, "IFCWALL", WALL_W, "Wall West", [0.0, 5.0, 0.0], [0.2, 10.0, 3.0], ".STANDARD.");
    let col = product(&mut s, "IFCCOLUMN", COLUMN, "Column C4", [8.0, 8.0, 0.0], [0.4, 0.4, 3.0], ".COLUMN.");
    let slab1 = product(&mut s, "IFCSLAB", "1Slb00000000000000001A", "Slab L1", [5.0, 5.0, -0.2], [10.0, 10.0, 0.2], ".FLOOR.");
    let door = product(&mut s, "IFCDOOR", DOOR, "Door 101", [2.0, 0.0, 0.0], [0.9, 0.25, 2.1], "2.1,0.9,.DOOR.,$,$");
    let room = product(&mut s, "IFCSPACE", "1Spc00000000000000101A", "101", [5.0, 5.0, 0.0], [9.0, 9.0, 3.0], ".ELEMENT.,.INTERNAL.,$");
    let duct = product(&mut s, "IFCDUCTSEGMENT", DUCT, "Supply Duct 3", [5.0, 3.0, 2.6], [6.0, 0.4, 0.3], ".RIGIDSEGMENT.");
    let w2 = product(&mut s, "IFCWALL", WALL_2, "Wall North L2", [5.0, 0.0, 3.5], [10.0, 0.2, 3.0], ".STANDARD.");
    let slab2 = product(&mut s, "IFCSLAB", "1Slb00000000000000002A", "Slab L2", [5.0, 5.0, 3.3], [10.0, 10.0, 0.2], ".FLOOR.");
    s.add(format!("IFCRELCONTAINEDINSPATIALSTRUCTURE('0Rel000000000000000004',$,$,$,(#{wn},#{ww},#{col},#{slab1},#{door}),#{l1})"));
    s.add(format!("IFCRELAGGREGATES('0Rel000000000000000005',$,$,$,#{l1},(#{room}))"));
    s.add(format!("IFCRELCONTAINEDINSPATIALSTRUCTURE('0Rel000000000000000006',$,$,$,(#{duct}),#{room})"));
    s.add(format!("IFCRELCONTAINEDINSPATIALSTRUCTURE('0Rel000000000000000007',$,$,$,(#{w2},#{slab2}),#{l2})"));
    // Walls: fire ratings and lengths.
    for (k, (w, rating, len)) in [(wn, "2HR", 10.0), (ww, "1HR", 10.0), (w2, "2HR", 10.0)].into_iter().enumerate() {
        let a = s.add(format!("IFCPROPERTYSINGLEVALUE('FireRating',$,IFCLABEL('{rating}'),$)"));
        let b = s.add("IFCPROPERTYSINGLEVALUE('IsExternal',$,IFCBOOLEAN(.T.),$)");
        let ps = s.add(format!("IFCPROPERTYSET('0Pst00000000000000000{k}',$,'Pset_WallCommon',$,(#{a},#{b}))"));
        let q = s.add(format!("IFCQUANTITYLENGTH('Length',$,$,{len:.1},$)"));
        let qs = s.add(format!("IFCELEMENTQUANTITY('0Qto00000000000000000{k}',$,'Qto_WallBaseQuantities',$,$,(#{q}))"));
        s.add(format!("IFCRELDEFINESBYPROPERTIES('0Rdp00000000000000000{k}',$,$,$,(#{w}),#{ps})"));
        s.add(format!("IFCRELDEFINESBYPROPERTIES('0Rdq00000000000000000{k}',$,$,$,(#{w}),#{qs})"));
    }
    let ty = s.add("IFCDUCTSEGMENTTYPE('0Typ000000000000000001',$,'Duct 16x12',$,$,$,$,$,$,.RIGIDSEGMENT.)");
    s.add(format!("IFCRELDEFINESBYTYPE('0Rel000000000000000008',$,$,$,(#{duct}),#{ty})"));
    let sys = s.add("IFCDISTRIBUTIONSYSTEM('0Sys000000000000000001',$,'Supply Air',$,$,$,.SUPPLYAIR.)");
    s.add(format!("IFCRELASSIGNSTOGROUP('0Rel000000000000000009',$,$,$,(#{duct}),$,#{sys})"));
    s.text()
}

fn open_hotel(name: &str) -> (Session, std::path::PathBuf) {
    hooks();
    let d = dir(name);
    let path = d.join("hotel.ifc");
    std::fs::write(&path, hotel_ifc()).unwrap();
    let mut s = Session::empty();
    let r = s.execute("ifc.open", &json!({"path": path.display().to_string()})).unwrap();
    assert_eq!(r["elements"], 9, "{r}");
    (s, d)
}

fn handle_of(s: &mut Session, guid: &str) -> String {
    let p = s.execute("ifc.properties", &json!({"guid": guid})).unwrap();
    p["handle"].as_str().unwrap().to_string()
}

fn names(v: &Value) -> Vec<String> {
    v.as_array().unwrap().iter().map(|n| n["name"].as_str().unwrap().to_string()).collect()
}

#[test]
fn the_model_opens_with_its_tree_properties_filters_section_box_and_measures() {
    let (mut s, d) = open_hotel("model");
    let r = s.execute("ifc.open", &json!({"path": d.join("hotel.ifc").display().to_string()})).unwrap();
    assert_eq!(r["storeys"], json!(["Level 1", "Level 2"]), "{r}");
    assert_eq!((r["sites"].clone(), r["buildings"].clone()), (json!(["Site A"]), json!(["Hotel B"])));
    assert_eq!(r["classes"]["Wall"], 3);
    // A model view: looking down from the south-west.
    let st = s.state().unwrap();
    assert!(st.view_dir.z > 0.0 && st.view_dir.x < 0.0, "{:?}", st.view_dir);

    // The spatial tree: site → building → storeys bottom up → room 101 → its duct.
    let t = s.execute("ifc.tree", &json!({})).unwrap();
    assert_eq!(t["elements"], 9);
    let site = &t["tree"][0];
    assert_eq!((site["kind"].as_str(), site["name"].as_str(), site["count"].as_u64()), (Some("site"), Some("Site A"), Some(9)));
    let bldg = &site["children"][0];
    assert_eq!(bldg["name"], "Hotel B");
    assert_eq!(names(&bldg["children"]), ["Level 1", "Level 2"]);
    let l1 = &bldg["children"][0];
    assert_eq!((l1["count"].as_u64(), l1["elevation"].as_f64()), (Some(7), Some(0.0)));
    let room = l1["children"].as_array().unwrap().iter().find(|c| c["kind"] == "space").unwrap();
    assert_eq!((room["name"].as_str(), room["count"].as_u64()), (Some("101"), Some(1)));
    assert_eq!(room["children"][0]["elements"][0]["guid"], DUCT);
    let walls = l1["children"].as_array().unwrap().iter().find(|c| c["kind"] == "class" && c["name"] == "Wall").unwrap();
    assert_eq!(walls["count"], 2);
    assert_eq!(bldg["children"][1]["elevation"], 3.5);

    // One object's properties: sets and quantities apart, where it is, its system and type.
    let w = s.execute("ifc.properties", &json!({"guid": WALL_N})).unwrap();
    assert_eq!(w["property_sets"]["Pset_WallCommon"]["FireRating"], "2HR", "{w}");
    assert_eq!(w["quantity_sets"]["Qto_WallBaseQuantities"]["Length"], "10");
    assert!(w["property_sets"].get("Qto_WallBaseQuantities").is_none());
    assert_eq!((w["storey"].as_str(), w["building"].as_str(), w["site"].as_str()), (Some("Level 1"), Some("Hotel B"), Some("Site A")));
    assert_eq!(w["size"], json!([10.0, 0.2, 3.0]));
    let duct = handle_of(&mut s, DUCT);
    s.execute("select", &json!({"handles": [duct]})).unwrap();
    let dp = s.execute("ifc.properties", &json!({})).unwrap();
    assert_eq!((dp["space"].as_str(), dp["type"].as_str(), dp["systems"].clone()), (Some("101"), Some("Duct 16x12"), json!(["Supply Air"])), "{dp}");

    // Filters: list, select by a property value, isolate a class, hide a storey, show all.
    let l2 = s.execute("ifc.filter", &json!({"level": "Level 2"})).unwrap();
    assert_eq!(l2["matched"], 2);
    let two_hr = s.execute("ifc.filter", &json!({"property": "Pset_WallCommon.FireRating=2hr", "action": "select"})).unwrap();
    assert_eq!(two_hr["matched"], 2);
    assert_eq!(s.selection().len(), 2);
    let iso = s.execute("ifc.filter", &json!({"class": "IfcWall", "action": "isolate"})).unwrap();
    assert_eq!((iso["matched"].as_u64(), iso["hidden"].as_u64()), (Some(3), Some(6)), "{iso}");
    assert_eq!(s.doc().unwrap().hidden_objects.len(), 6);
    // Isolating again shows what the last one hid first.
    let iso = s.execute("ifc.filter", &json!({"system": "supply air", "action": "isolate"})).unwrap();
    assert_eq!(iso["hidden"], 8);
    let shown = s.execute("ifc.filter", &json!({"action": "show"})).unwrap();
    assert_eq!(shown["shown"], 8);
    assert!(s.doc().unwrap().hidden_objects.is_empty());
    let hid = s.execute("ifc.filter", &json!({"level": "Level 2", "action": "hide"})).unwrap();
    assert_eq!(hid["hidden"], 2);
    s.execute("ifc.filter", &json!({"action": "show"})).unwrap();
    assert!(s.execute("ifc.filter", &json!({"action": "isolate"})).is_err(), "no filter");
    assert!(s.execute("ifc.filter", &json!({"class": "Wall", "action": "explode"})).is_err());

    // The section box round the column: what is wholly outside it hides, and comes back.
    let col = handle_of(&mut s, COLUMN);
    s.execute("select", &json!({"handles": [col]})).unwrap();
    let sb = s.execute("ifc.sectionbox", &json!({"fit": "selection", "pad": 0.4})).unwrap();
    assert_eq!(sb["on"], true);
    assert_eq!((sb["min"].clone(), sb["max"].clone()), (json!([7.4, 7.4, -0.4]), json!([8.6, 8.6, 3.4])), "{sb}");
    let hidden: Vec<String> = s.doc().unwrap().hidden_objects.iter().map(|h| h.hex()).collect();
    // The column, the slabs and the room (which reach into the box) stay; the walls, the door and
    // the duct go.
    assert_eq!(hidden.len(), 5, "{sb}");
    assert!(!hidden.contains(&col));
    let off = s.execute("ifc.sectionbox", &json!({"on": false})).unwrap();
    assert_eq!(off["on"], false);
    assert!(s.doc().unwrap().hidden_objects.is_empty());
    assert!(s.execute("ifc.sectionbox", &json!({"min": [0, 0, 0], "max": [0, 0, 0]})).is_err());

    // Measuring: points, and the clear gap between the column and the north wall.
    let m = s.execute("ifc.measure", &json!({"from": [0, 0, 0], "to": [3, 4, 0]})).unwrap();
    assert_eq!((m["distance"].as_f64(), m["metres"].as_f64()), (Some(5.0), Some(5.0)));
    let g = s.execute("ifc.measure", &json!({"a": COLUMN, "b": WALL_N})).unwrap();
    assert!((g["gap"].as_f64().unwrap() - 7.7).abs() < 1e-6, "{g}");
    assert!(s.execute("ifc.measure", &json!({"from": [0, 0, 0]})).is_err());
    s.execute("select", &json!({"handles": [col, duct]})).unwrap();
    let g = s.execute("ifc.measure", &json!({})).unwrap();
    assert_eq!(g["a"]["guid"], COLUMN);

    // A render: the duct marked, seen from a camera, cut at a section box.
    s.execute("select", &json!({"handles": [duct]})).unwrap();
    s.execute("ifc.camera", &json!({"eye": [-6, -9, 9], "target": [5, 4, 1]})).unwrap();
    s.execute("ifc.sectionbox", &json!({"min": [-1, -1, -1], "max": [11, 11, 3.2]})).unwrap();
    let png = d.join("render.png");
    let r = s.execute("ifc.render", &json!({"path": png.display().to_string(), "width": 640, "height": 480})).unwrap();
    assert!(r["triangles"].as_u64().unwrap() > 50, "{r}");
    let bytes = std::fs::read(&png).unwrap();
    assert!(bytes.starts_with(&[0x89, b'P', b'N', b'G']));
    if let Ok(keep) = std::env::var("IFCVIEWER_SHOTS") {
        std::fs::copy(&png, std::path::Path::new(&keep).join("ifc-render.png")).unwrap();
    }

    // Walk: the storeys as levels, the walls and column to bump into, spaces left out.
    let scene = ifcmodel::walk_scene(s.doc().unwrap()).unwrap();
    assert_eq!(scene.levels.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), ["Level 1", "Level 2"]);
    assert!((scene.levels[1].elevation_ft - 3.5 / 0.3048).abs() < 1e-6);
    assert!(scene.walls.len() >= 12, "{}", scene.walls.len());
    assert!(scene.walls.iter().all(|w| w.level <= 1));
    assert!(!scene.tris.is_empty() && scene.owners.len() * 3 == scene.tris.len());
    let room_h = handle_of(&mut s, "1Spc00000000000000101A");
    assert!(!scene.owners.iter().any(|o| o.handle.hex() == room_h), "spaces aren't walls");
}

#[test]
fn bcf_issues_go_round_trip_between_sessions_and_stay_in_the_drawing() {
    let (mut a, d) = open_hotel("bcf");
    let duct = handle_of(&mut a, DUCT);
    a.execute("select", &json!({"handles": [duct]})).unwrap();
    a.execute("ifc.camera", &json!({"eye": [1, 7, 1.7], "target": [5, 3, 2.6]})).unwrap();
    let t = a
        .execute("bcf.create", &json!({"title": "Duct hits the north wall's head?", "comment": "Check the duct's bottom against the door head.", "assignee": "mech@example.com", "priority": "High", "labels": ["Mechanical"]}))
        .unwrap();
    assert_eq!(
        (t["index"].as_u64(), t["status"].as_str(), t["selected"].as_u64(), t["snapshot"].as_bool()),
        (Some(1), Some("Open"), Some(1), Some(true)),
        "{t}"
    );
    let guid = t["guid"].as_str().unwrap().to_string();
    // A second, from the walls, no snapshot.
    a.execute("ifc.filter", &json!({"class": "wall", "level": "Level 1", "action": "select"})).unwrap();
    a.execute("bcf.create", &json!({"title": "Fire rating differs between walls", "snapshot": false, "type": "Request"})).unwrap();
    let list = a.execute("bcf.list", &json!({})).unwrap();
    assert_eq!(list["total"], 2);
    assert_eq!(a.execute("bcf.list", &json!({"assignee": "MECH@example.com"})).unwrap()["total"], 1);
    assert_eq!(a.execute("bcf.list", &json!({"text": "door head"})).unwrap()["total"], 1, "comments are searched");
    let got = a.execute("bcf.get", &json!({"topic": "1"})).unwrap();
    assert_eq!(got["viewpoints"][0]["selection"], json!([DUCT]));
    assert_eq!(got["viewpoints"][0]["eye"], json!([1.0, 7.0, 1.7]), "metres, model coordinates");
    let dir = &got["viewpoints"][0]["dir"];
    assert!((dir[1].as_f64().unwrap() + 4.0 / (32.0f64 + 0.81).sqrt()).abs() < 1e-9, "{dir}");
    if let Ok(keep) = std::env::var("IFCVIEWER_SHOTS") {
        let full = a.execute("bcf.get", &json!({"topic": guid, "snapshot": true})).unwrap();
        let b = cadaddy_engine::cmd::file::base64_decode(full["viewpoints"][0]["snapshot"].as_str().unwrap()).unwrap();
        std::fs::write(std::path::Path::new(&keep).join("bcf-snapshot.png"), b).unwrap();
    }
    let up = a.execute("bcf.update", &json!({"topic": guid, "status": "In Progress", "comment": "Raised 4\" in the next model."})).unwrap();
    assert_eq!((up["changed"].as_bool(), up["comments"].as_u64()), (Some(true), Some(2)));
    assert_eq!(a.execute("bcf.update", &json!({"topic": guid, "status": "In Progress"})).unwrap()["changed"], false);
    assert!(a.execute("bcf.update", &json!({"topic": guid, "title": ""})).is_err());

    // Export; another session opens the same model and imports it.
    let file = d.join("issues.bcf");
    let ex = a.execute("bcf.export", &json!({"path": file.display().to_string()})).unwrap();
    assert_eq!(ex["topics"], 2);
    let (mut b, _) = open_hotel("bcf-b");
    let im = b.execute("bcf.import", &json!({"path": file.display().to_string()})).unwrap();
    assert_eq!((im["added"].as_u64(), im["skipped"].as_array().map(Vec::len)), (Some(2), Some(0)), "{im}");
    let bt = b.execute("bcf.get", &json!({"topic": guid})).unwrap();
    assert_eq!(
        (bt["status"].as_str(), bt["assignee"].as_str(), bt["priority"].as_str()),
        (Some("In Progress"), Some("mech@example.com"), Some("High"))
    );
    assert_eq!(bt["labels"], json!(["Mechanical"]));
    assert_eq!(bt["comments"].as_array().unwrap().len(), 2);
    assert!(bt["viewpoints"][0]["snapshot"].is_string());
    // Shown: its object selected, its camera back.
    b.set_selection(Vec::new());
    let shown = b.execute("bcf.show", &json!({"topic": guid})).unwrap();
    assert_eq!(shown["selected"], json!([handle_of(&mut b, DUCT)]));
    assert_eq!(shown["missing"], json!([]));
    assert_eq!(shown["eye"], json!([1.0, 7.0, 1.7]));
    assert_eq!(b.selection().len(), 1);
    // Answered there, sent back: the comments merge and the later status wins.
    b.execute("bcf.update", &json!({"topic": guid, "status": "Resolved", "comment": "Done in rev 3.", "author": "mech@example.com"})).unwrap();
    let back = d.join("answered.bcf");
    b.execute("bcf.export", &json!({"path": back.display().to_string(), "topics": [guid.clone()]})).unwrap();
    let im = a.execute("bcf.import", &json!({"path": back.display().to_string()})).unwrap();
    assert_eq!((im["added"].as_u64(), im["updated"].as_u64()), (Some(0), Some(1)), "{im}");
    let at = a.execute("bcf.get", &json!({"topic": guid})).unwrap();
    assert_eq!(at["status"], "Resolved");
    assert_eq!(at["comments"].as_array().unwrap().len(), 3);
    assert_eq!(a.execute("bcf.import", &json!({"path": back.display().to_string()})).unwrap()["unchanged"], 1, "again: nothing new");

    // Kept in the drawing: saved as .cdy and DXF, opened again.
    for ext in ["cdy", "dxf"] {
        let p = d.join(format!("hotel-issues.{ext}"));
        a.execute("saveas", &json!({"path": p.display().to_string()})).unwrap();
        let mut c = Session::empty();
        c.execute("open", &json!({"path": p.display().to_string()})).unwrap();
        let l = c.execute("bcf.list", &json!({})).unwrap();
        assert_eq!(l["total"], 2, "{ext}");
        let ct = c.execute("bcf.get", &json!({"topic": guid, "snapshot": true})).unwrap();
        assert_eq!(ct["comments"].as_array().unwrap().len(), 3, "{ext}");
        assert!(ct["viewpoints"][0]["snapshot"].as_str().unwrap().len() > 100, "{ext}");
        assert_eq!(c.execute("ifc.properties", &json!({"guid": DUCT})).unwrap()["space"], "101", "{ext}: the IFC tags survive");
    }

    // Undo takes a new topic back; delete removes one.
    a.execute("bcf.create", &json!({"title": "Temporary", "snapshot": false})).unwrap();
    assert_eq!(a.execute("bcf.list", &json!({})).unwrap()["total"], 3);
    a.execute("undo", &json!({})).unwrap();
    assert_eq!(a.execute("bcf.list", &json!({})).unwrap()["total"], 2);
    a.execute("bcf.delete", &json!({"topic": "2"})).unwrap();
    assert_eq!(a.execute("bcf.list", &json!({})).unwrap()["total"], 1);
}

/// A BCF file as another program might write it: a header, namespaces, an orthogonal camera,
/// labels, a JPEG snapshot, CDATA and entities; and a BCF 2.0 topic that doesn't list its viewpoint.
fn foreign_bcf() -> Vec<u8> {
    let markup = r#"<?xml version="1.0" encoding="utf-8"?>
<Markup xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xmlns:xsd="http://www.w3.org/2001/XMLSchema">
  <Header><File IfcProject="0Prj000000000000000001" isExternal="true"><Filename>hotel.ifc</Filename></File></Header>
  <Topic Guid="a1b2c3d4-0000-4000-8000-000000000001" TopicType="Clash" TopicStatus="Open">
    <ReferenceLink>https://example.com/issues/7</ReferenceLink>
    <Title>Column &amp; slab edge &lt;150 mm&gt;</Title>
    <Priority>Normal</Priority>
    <Index>7</Index>
    <Labels>Structure</Labels>
    <Labels>Level 1</Labels>
    <CreationDate>2026-10-01T09:30:00+00:00</CreationDate>
    <CreationAuthor>arch@example.com</CreationAuthor>
    <AssignedTo>struct@example.com</AssignedTo>
    <Description><![CDATA[Is the column <inside> the slab edge?]]></Description>
  </Topic>
  <Comment Guid="c0000000-0000-4000-8000-000000000001">
    <Date>2026-10-01T09:31:00+00:00</Date>
    <Author>arch@example.com</Author>
    <Comment>See the view.</Comment>
    <Viewpoint Guid="v0000000-0000-4000-8000-000000000001" />
  </Comment>
  <Viewpoints Guid="v0000000-0000-4000-8000-000000000001">
    <Viewpoint>view-a.bcfv</Viewpoint>
    <Snapshot>view-a.jpg</Snapshot>
  </Viewpoints>
</Markup>"#;
    let view = format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<VisualizationInfo Guid="v0000000-0000-4000-8000-000000000001" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
  <Components>
    <ViewSetupHints SpacesVisible="false" />
    <Selection><Component IfcGuid="{COLUMN}" /><Component IfcGuid="0NotInThisModel00000001" /></Selection>
    <Visibility DefaultVisibility="true" />
  </Components>
  <OrthogonalCamera>
    <CameraViewPoint><X>8</X><Y>-2</Y><Z>1.5</Z></CameraViewPoint>
    <CameraDirection><X>0</X><Y>1</Y><Z>0</Z></CameraDirection>
    <CameraUpVector><X>0</X><Y>0</Y><Z>1</Z></CameraUpVector>
    <ViewToWorldScale>6</ViewToWorldScale>
  </OrthogonalCamera>
</VisualizationInfo>"#
    );
    let old_markup = r#"<?xml version="1.0"?><Markup><Topic Guid="b0000000-0000-4000-8000-000000000002"><Title>Old style</Title></Topic></Markup>"#;
    let old_view = r#"<VisualizationInfo><PerspectiveCamera><CameraViewPoint><X>0</X><Y>0</Y><Z>10</Z></CameraViewPoint><CameraDirection><X>0</X><Y>0</Y><Z>-1</Z></CameraDirection><CameraUpVector><X>0</X><Y>1</Y><Z>0</Z></CameraUpVector><FieldOfView>45</FieldOfView></PerspectiveCamera></VisualizationInfo>"#;
    let jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0, 0x10, b'J', b'F', b'I', b'F', 0, 0xFF, 0xD9];
    let files = vec![
        ("bcf.version".to_string(), br#"<?xml version="1.0"?><Version VersionId="2.1"><DetailedVersion>2.1</DetailedVersion></Version>"#.to_vec()),
        ("project.bcfp".to_string(), b"<ProjectExtension><Project ProjectId=\"p\"><Name>Hotel</Name></Project></ProjectExtension>".to_vec()),
        ("a1b2c3d4-0000-4000-8000-000000000001/markup.bcf".to_string(), markup.as_bytes().to_vec()),
        ("a1b2c3d4-0000-4000-8000-000000000001/view-a.bcfv".to_string(), view.into_bytes()),
        ("a1b2c3d4-0000-4000-8000-000000000001/view-a.jpg".to_string(), jpeg),
        ("b0000000-0000-4000-8000-000000000002/markup.bcf".to_string(), old_markup.as_bytes().to_vec()),
        ("b0000000-0000-4000-8000-000000000002/viewpoint.bcfv".to_string(), old_view.as_bytes().to_vec()),
    ];
    cadaddy_engine::cmd::file::zip::write(&files).unwrap()
}

#[test]
fn another_programs_bcf_reads_and_junk_is_refused() {
    let (mut s, d) = open_hotel("foreign");
    let file = d.join("from-arch.bcfzip");
    std::fs::write(&file, foreign_bcf()).unwrap();
    let im = s.execute("bcf.import", &json!({"path": file.display().to_string()})).unwrap();
    assert_eq!(im["added"], 2, "{im}");
    let t = s.execute("bcf.get", &json!({"topic": "a1b2c3d4"})).unwrap();
    assert_eq!(t["title"], "Column & slab edge <150 mm>");
    assert_eq!((t["topic_type"].as_str(), t["index"].as_u64(), t["assignee"].as_str()), (Some("Clash"), Some(7), Some("struct@example.com")));
    assert_eq!(t["labels"], json!(["Structure", "Level 1"]));
    assert_eq!(t["description"], "Is the column <inside> the slab edge?");
    assert_eq!(t["comments"][0]["viewpoint"], "v0000000-0000-4000-8000-000000000001");
    let vp = &t["viewpoints"][0];
    assert_eq!((vp["ortho"].as_bool(), vp["fov"].as_f64(), vp["snapshot_type"].as_str()), (Some(true), Some(6.0), Some("jpg")));
    let shown = s.execute("bcf.show", &json!({"topic": "7"})).unwrap();
    assert_eq!(shown["selected"], json!([handle_of(&mut s, COLUMN)]));
    assert_eq!(shown["missing"], json!(["0NotInThisModel00000001"]), "an object of another model is reported, not an error");
    assert_eq!(shown["target"], json!([8.0, 8.0, 1.5]), "as far along the view as the column");
    let old = s.execute("bcf.get", &json!({"topic": "b0000000-0000-4000-8000-000000000002"})).unwrap();
    assert_eq!(old["viewpoints"][0]["fov"], 45.0, "BCF 2.0: the topic folder's viewpoint.bcfv");
    // Ours written back out reads in as the same topics.
    let out = d.join("again.bcf");
    s.execute("bcf.export", &json!({"path": out.display().to_string()})).unwrap();
    let (topics, skipped) = cadaddy_engine::cmd::bcf::read_bcf(&std::fs::read(&out).unwrap()).unwrap();
    assert!(skipped.is_empty());
    assert_eq!(topics, cadaddy_engine::cmd::bcf::load(s.doc().unwrap()).topics);

    // Junk: not a zip, a zip without topics, a newer version, a markup without a topic, a bad
    // viewpoint, hostile params. Nothing panics; nothing half-imports.
    let junk = d.join("junk.bcf");
    for bytes in [b"not a zip at all".to_vec(), cadaddy_engine::cmd::file::zip::write(&[("readme.txt".into(), b"hi".to_vec())]).unwrap()] {
        std::fs::write(&junk, bytes).unwrap();
        assert!(s.execute("bcf.import", &json!({"path": junk.display().to_string()})).is_err());
    }
    let v3 = cadaddy_engine::cmd::file::zip::write(&[("bcf.version".into(), br#"<Version VersionId="3.0"/>"#.to_vec())]).unwrap();
    assert!(cadaddy_engine::cmd::bcf::read_bcf(&v3).unwrap_err().contains("3.0"));
    let broken = cadaddy_engine::cmd::file::zip::write(&[
        ("x/markup.bcf".into(), b"<Markup><Comment>no topic".to_vec()),
        ("y/markup.bcf".into(), br#"<Markup><Topic Guid="y"><Title>T</Title></Topic><Viewpoints><Viewpoint>v.bcfv</Viewpoint><Snapshot>s.png</Snapshot></Viewpoints></Markup>"#.to_vec()),
        ("y/v.bcfv".into(), b"<VisualizationInfo><PerspectiveCamera><CameraViewPoint><X>nan</X></CameraViewPoint></PerspectiveCamera>".to_vec()),
        ("y/s.png".into(), b"GIF89a".to_vec()),
    ])
    .unwrap();
    let (t, skipped) = cadaddy_engine::cmd::bcf::read_bcf(&broken).unwrap();
    assert_eq!(t.len(), 1);
    assert_eq!(skipped.len(), 2, "{skipped:?}");
    let before = cadaddy_engine::cmd::bcf::load(s.doc().unwrap());
    for p in [
        json!({}),
        json!({"data": "%%%"}),
        json!({"path": d.join("missing.bcf").display().to_string()}),
        json!({"data": cadaddy_engine::cmd::file::base64_encode(&[0x50, 0x4B, 5, 6, 0, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0, 0])}),
    ] {
        assert!(s.execute("bcf.import", &p).is_err(), "{p}");
    }
    assert_eq!(cadaddy_engine::cmd::bcf::load(s.doc().unwrap()), before);
    for (cmd, p) in [
        ("bcf.create", json!({})),
        ("bcf.create", json!({"title": "x", "eye": [0, 0, 0], "target": [0, 0, 0]})),
        ("bcf.create", json!({"title": "x", "eye": ["a", 1, 2], "target": [1, 2, 3]})),
        ("bcf.get", json!({"topic": "nope"})),
        ("bcf.show", json!({"topic": 1e300})),
        ("bcf.update", json!({})),
        ("bcf.export", json!({})),
        ("bcf.export", json!({"path": d.join("x.bcf").display().to_string(), "topics": ["nope"]})),
        ("ifc.properties", json!({"guid": "nope"})),
        ("ifc.measure", json!({"a": COLUMN})),
        ("ifc.camera", json!({"eye": [1, 1, 1], "target": [1, 1, 1]})),
        ("ifc.render", json!({"eye": [f64::MAX, 0, 0], "target": [0, 0, 0]})),
        ("ifc.sectionbox", json!({"fit": "everything"})),
        ("ifc.sectionbox", json!({"fit": "all", "pad": -1})),
        ("ifc.open", json!({"path": "plan.dxf"})),
        ("ifc.open", json!({})),
    ] {
        assert!(s.execute(cmd, &p).is_err(), "{cmd} {p}");
    }
    // A long title is cut, not refused; a huge width is clamped.
    let long = "x".repeat(50_000);
    let r = s.execute("bcf.create", &json!({"title": long, "width": u64::MAX, "height": 0})).unwrap();
    assert_eq!(r["title"].as_str().unwrap().len(), 8_000);
}

#[test]
fn the_viewer_commands_say_what_they_need_without_a_model() {
    let mut s = Session::empty();
    s.new_drawing(false);
    for cmd in ["ifc.tree", "ifc.properties", "ifc.sectionbox", "ifc.render"] {
        let e = s.execute(cmd, &json!({})).unwrap_err().to_string();
        assert!(e.contains("open an IFC model"), "{cmd}: {e}");
    }
    assert_eq!(s.execute("bcf.list", &json!({})).unwrap()["total"], 0);
    assert!(ifcmodel::walk_scene(s.doc().unwrap()).is_none());
    // A topic can still be raised on a plain drawing (no objects, no snapshot).
    let t = s.execute("bcf.create", &json!({"title": "Plan question"})).unwrap();
    assert_eq!((t["selected"].as_u64(), t["snapshot"].as_bool()), (Some(0), Some(false)));
}
