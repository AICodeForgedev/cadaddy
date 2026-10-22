//! Handover (Q286): COBie and schedules from a made-up hotel corner (an IFC4 model written here):
//! every sheet and its references, the BCF issue on the Issue sheet, our COBie validator passing it
//! and catching what a broken workbook gets wrong, the room/door/equipment schedules, and a model
//! CADaddy built itself (through its IFC export).

use cadaddy_engine::Session;
use cadaddy_engine::cmd::cobie::{self, Book};
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
    let d = std::env::temp_dir().join(format!("cadaddy-cobie-{name}-{}", std::process::id()));
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

const FCU: &str = "1Fcu00000000000000001A";

/// A hotel corner: Level 1 with rooms 101 (4.5 × 4.5 m, its area left to the geometry) and 102
/// (its gross area given, 19.5 m², a guest room), the wall and a door between them, 102's own
/// door, a fan coil unit in 101 (type FCU-1, Acme Air FC-400, chilled water), a supply duct in
/// 101, two lights and a basin in 102; Level 2 (3.5 m) with room 201 and a bed.
fn hotel_ifc() -> String {
    let mut s = Step::default();
    let unit = s.add("IFCSIUNIT(*,.LENGTHUNIT.,$,.METRE.)");
    let units = s.add(format!("IFCUNITASSIGNMENT((#{unit}))"));
    let project = s.add(format!("IFCPROJECT('0Prj000000000000000001',$,'Hotel',$,$,$,$,$,#{units})"));
    let site = s.add("IFCSITE('0Sit000000000000000001',$,'Riverside Lot',$,$,$,$,$,.ELEMENT.,$,$,$,$,$)");
    let bldg = s.add("IFCBUILDING('0Bld000000000000000001',$,'Harbor Hotel',$,$,$,$,$,.ELEMENT.,$,$,$)");
    let l1 = s.add("IFCBUILDINGSTOREY('0Sty000000000000000001',$,'Level 1',$,$,$,$,$,.ELEMENT.,0.)");
    let l2 = s.add("IFCBUILDINGSTOREY('0Sty000000000000000002',$,'Level 2',$,$,$,$,$,.ELEMENT.,3.5)");
    s.add(format!("IFCRELAGGREGATES('0Rel000000000000000001',$,$,$,#{project},(#{site}))"));
    s.add(format!("IFCRELAGGREGATES('0Rel000000000000000002',$,$,$,#{site},(#{bldg}))"));
    s.add(format!("IFCRELAGGREGATES('0Rel000000000000000003',$,$,$,#{bldg},(#{l1},#{l2}))"));
    let product = |s: &mut Step, entity: &str, guid: &str, name: &str, at: [f64; 3], size: [f64; 3], tail: &str| {
        let (place, pds) = s.boxed(at, size);
        s.add(format!("{entity}('{guid}',$,'{name}',$,$,#{place},#{pds},$,{tail})"))
    };
    let r101 = product(&mut s, "IFCSPACE", "1Spc00000000000000101A", "101", [2.25, 2.25, 0.0], [4.5, 4.5, 3.0], ".ELEMENT.,.INTERNAL.,$");
    let r102 = product(&mut s, "IFCSPACE", "1Spc00000000000000102A", "102", [6.95, 2.25, 0.0], [4.5, 4.5, 3.0], ".ELEMENT.,.INTERNAL.,$");
    let r201 = product(&mut s, "IFCSPACE", "1Spc00000000000000201A", "201", [2.25, 2.25, 3.5], [4.5, 4.5, 3.0], ".ELEMENT.,.INTERNAL.,$");
    let wall = product(&mut s, "IFCWALL", "1Wal00000000000000001A", "Wall 101/102", [4.6, 2.25, 0.0], [0.2, 4.5, 3.0], ".STANDARD.");
    let d101 = product(&mut s, "IFCDOOR", "1Dor00000000000000101A", "D101", [4.6, 1.0, 0.0], [0.25, 0.9, 2.1], "2.1,0.9,.DOOR.,$,$");
    let d102 = product(&mut s, "IFCDOOR", "1Dor00000000000000102A", "D102", [6.95, 4.6, 0.0], [0.9, 0.25, 2.1], "2.1,0.9,.DOOR.,$,$");
    let fcu = product(&mut s, "IFCUNITARYEQUIPMENT", FCU, "FCU-101", [1.0, 1.0, 2.4], [1.0, 0.5, 0.3], ".NOTDEFINED.");
    let duct = product(&mut s, "IFCDUCTSEGMENT", "1Dct00000000000000001A", "Duct 101", [2.25, 3.5, 2.6], [3.0, 0.4, 0.3], ".RIGIDSEGMENT.");
    let lt1 = product(&mut s, "IFCLIGHTFIXTURE", "1Lgt00000000000000001A", "Light", [6.0, 2.0, 2.9], [0.6, 0.6, 0.1], ".POINTSOURCE.");
    let lt2 = product(&mut s, "IFCLIGHTFIXTURE", "1Lgt00000000000000002A", "Light", [8.0, 2.0, 2.9], [0.6, 0.6, 0.1], ".POINTSOURCE.");
    let basin = product(&mut s, "IFCSANITARYTERMINAL", "1San00000000000000001A", "Basin 102", [8.5, 4.0, 0.8], [0.5, 0.4, 0.2], ".WASHHANDBASIN.");
    let bed = product(&mut s, "IFCFURNITURE", "1Fur00000000000000001A", "Bed 201", [2.0, 2.0, 3.5], [2.0, 1.6, 0.5], ".NOTDEFINED.");
    let slab1 = product(&mut s, "IFCSLAB", "1Slb00000000000000001A", "Slab L1", [4.6, 2.25, -0.2], [9.4, 4.7, 0.2], ".FLOOR.");
    let slab2 = product(&mut s, "IFCSLAB", "1Slb00000000000000002A", "Slab L2", [4.6, 2.25, 3.3], [9.4, 4.7, 0.2], ".FLOOR.");
    s.add(format!("IFCRELAGGREGATES('0Rel000000000000000004',$,$,$,#{l1},(#{r101},#{r102}))"));
    s.add(format!("IFCRELAGGREGATES('0Rel000000000000000005',$,$,$,#{l2},(#{r201}))"));
    s.add(format!(
        "IFCRELCONTAINEDINSPATIALSTRUCTURE('0Rel000000000000000006',$,$,$,(#{wall},#{d101},#{d102},#{fcu},#{lt1},#{basin},#{slab1}),#{l1})"
    ));
    s.add(format!("IFCRELCONTAINEDINSPATIALSTRUCTURE('0Rel000000000000000007',$,$,$,(#{duct}),#{r101})"));
    s.add(format!("IFCRELCONTAINEDINSPATIALSTRUCTURE('0Rel000000000000000008',$,$,$,(#{lt2}),#{r102})"));
    s.add(format!("IFCRELCONTAINEDINSPATIALSTRUCTURE('0Rel000000000000000009',$,$,$,(#{bed},#{slab2}),#{l2})"));
    // Room 102: a guest room whose area the model states.
    let cat = s.add("IFCPROPERTYSINGLEVALUE('Category',$,IFCLABEL('Guest Room'),$)");
    let ps = s.add(format!("IFCPROPERTYSET('0Pst000000000000000001',$,'Pset_SpaceCommon',$,(#{cat}))"));
    s.add(format!("IFCRELDEFINESBYPROPERTIES('0Rdp000000000000000001',$,$,$,(#{r102}),#{ps})"));
    let area = s.add("IFCQUANTITYAREA('GrossFloorArea',$,$,19.5,$)");
    let qs = s.add(format!("IFCELEMENTQUANTITY('0Qto000000000000000001',$,'Qto_SpaceBaseQuantities',$,$,(#{area}))"));
    s.add(format!("IFCRELDEFINESBYPROPERTIES('0Rdq000000000000000001',$,$,$,(#{r102}),#{qs})"));
    // Doors: fire ratings.
    for (k, (d, rating)) in [(d101, "20 MIN"), (d102, "45 MIN")].into_iter().enumerate() {
        let f = s.add(format!("IFCPROPERTYSINGLEVALUE('FireRating',$,IFCLABEL('{rating}'),$)"));
        let ps = s.add(format!("IFCPROPERTYSET('0Psd00000000000000000{k}',$,'Pset_DoorCommon',$,(#{f}))"));
        s.add(format!("IFCRELDEFINESBYPROPERTIES('0Rdd00000000000000000{k}',$,$,$,(#{d}),#{ps})"));
    }
    // The fan coil unit's type and maker; the lights' type.
    let mfr = s.add("IFCPROPERTYSINGLEVALUE('Manufacturer',$,IFCLABEL('Acme Air'),$)");
    let model = s.add("IFCPROPERTYSINGLEVALUE('ModelLabel',$,IFCLABEL('FC-400'),$)");
    let mps = s.add(format!("IFCPROPERTYSET('0Pst000000000000000002',$,'Pset_ManufacturerTypeInformation',$,(#{mfr},#{model}))"));
    let fty = s.add(format!("IFCUNITARYEQUIPMENTTYPE('0Typ000000000000000001',$,'FCU-1',$,$,(#{mps}),$,$,$,.NOTDEFINED.)"));
    s.add(format!("IFCRELDEFINESBYTYPE('0Rel000000000000000010',$,$,$,(#{fcu}),#{fty})"));
    let lty = s.add("IFCLIGHTFIXTURETYPE('0Typ000000000000000002',$,'LT-A',$,$,$,$,$,$,.POINTSOURCE.)");
    s.add(format!("IFCRELDEFINESBYTYPE('0Rel000000000000000011',$,$,$,(#{lt1},#{lt2}),#{lty})"));
    let chw = s.add("IFCDISTRIBUTIONSYSTEM('0Sys000000000000000001',$,'Chilled Water',$,$,$,.CHILLEDWATER.)");
    s.add(format!("IFCRELASSIGNSTOGROUP('0Rel000000000000000012',$,$,$,(#{fcu}),$,#{chw})"));
    let sa = s.add("IFCDISTRIBUTIONSYSTEM('0Sys000000000000000002',$,'Supply Air',$,$,$,.SUPPLYAIR.)");
    s.add(format!("IFCRELASSIGNSTOGROUP('0Rel000000000000000013',$,$,$,(#{fcu},#{duct}),$,#{sa})"));
    s.text()
}

fn hotel(name: &str) -> (Session, std::path::PathBuf) {
    hooks();
    let d = dir(name);
    let path = d.join("hotel.ifc");
    std::fs::write(&path, hotel_ifc()).unwrap();
    let mut s = Session::empty();
    s.execute("ifc.open", &json!({"path": path.display().to_string()})).unwrap();
    (s, d)
}

const ON: &str = "2026-10-21T09:00:00";

fn export(s: &mut Session, path: &std::path::Path) -> (Value, Book) {
    let r = s
        .execute(
            "cobie.export",
            &json!({"path": path.display().to_string(), "author": "owner@example.com", "company": "Made-up Hotels", "project": "Hotel corner", "category": "Hotel", "phase": "Handover", "created_on": ON}),
        )
        .unwrap();
    (r, Book::read(&std::fs::read(path).unwrap()).unwrap())
}

/// The values of `col` in a sheet's data rows.
fn col(b: &Book, sheet: &str, col: &str) -> Vec<String> {
    b.column(sheet, col).into_iter().map(|(_, v)| v).collect()
}

#[test]
fn cobie_from_the_hotel_has_every_sheet_and_passes_its_rules() {
    let (mut s, d) = hotel("export");
    let fcu = s.execute("ifc.properties", &json!({"guid": FCU})).unwrap();
    s.execute("select", &json!({"handles": [fcu["handle"]]})).unwrap();
    s.execute("bcf.create", &json!({"title": "FCU access panel", "comment": "Keep 600 mm clear below the unit.", "assignee": "mech@example.com", "author": "arch@example.com", "priority": "High", "snapshot": false})).unwrap();
    let file = d.join("hotel-cobie.xlsx");
    let (r, b) = export(&mut s, &file);
    assert_eq!(r["validation"]["errors"], 0, "{r}");
    assert_eq!(r["source"], "the IFC model");
    let names: Vec<&str> = b.sheets.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, cobie::SHEETS.iter().map(|(n, _)| *n).collect::<Vec<_>>());
    for (n, cols) in cobie::SHEETS {
        assert_eq!(b.sheet(n).unwrap()[0], cols.iter().map(|c| c.to_string()).collect::<Vec<_>>(), "{n} header");
    }
    // Contacts: the author and the issue's people.
    assert_eq!(col(&b, "Contact", "Email"), ["arch@example.com", "mech@example.com", "owner@example.com"]);
    assert_eq!(b.value("Contact", "owner@example.com", "Company").as_deref(), Some("Made-up Hotels"));
    // Facility.
    assert_eq!(col(&b, "Facility", "Name"), ["Harbor Hotel"]);
    for (c, v) in [
        ("ProjectName", "Hotel corner"),
        ("SiteName", "Riverside Lot"),
        ("LinearUnits", "meters"),
        ("Category", "Hotel"),
        ("Phase", "Handover"),
        ("CreatedOn", ON),
    ] {
        assert_eq!(b.value("Facility", "Harbor Hotel", c).as_deref(), Some(v), "{c}");
    }
    // Floors bottom up, their elevations and heights.
    assert_eq!(col(&b, "Floor", "Name"), ["Level 1", "Level 2"]);
    assert_eq!(col(&b, "Floor", "Elevation"), ["0", "3.5"]);
    assert_eq!(col(&b, "Floor", "Height"), ["3.5", "n/a"]);
    // Spaces: areas from the geometry, or as the model states them; their floors.
    assert_eq!(col(&b, "Space", "Name"), ["101", "102", "201"]);
    assert_eq!(b.value("Space", "101", "GrossArea").as_deref(), Some("20.25"));
    assert_eq!(b.value("Space", "102", "GrossArea").as_deref(), Some("19.5"));
    assert_eq!(b.value("Space", "102", "Category").as_deref(), Some("Guest Room"));
    assert_eq!(b.value("Space", "201", "FloorName").as_deref(), Some("Level 2"));
    assert_eq!(b.value("Space", "101", "UsableHeight").as_deref(), Some("3"));
    // Types: one per type the components use; makers and models; furniture moves.
    let types = col(&b, "Type", "Name");
    for t in ["FCU-1", "LT-A", "Door", "SanitaryTerminal", "Furniture", "DuctSegment"] {
        assert!(types.contains(&t.to_string()), "{t} in {types:?}");
    }
    assert!(!types.iter().any(|t| t == "Wall" || t == "Slab"), "fabric isn't a type to maintain");
    assert_eq!(b.value("Type", "FCU-1", "Manufacturer").as_deref(), Some("Acme Air"));
    assert_eq!(b.value("Type", "FCU-1", "ModelNumber").as_deref(), Some("FC-400"));
    assert_eq!(b.value("Type", "FCU-1", "NominalLength").as_deref(), Some("1"));
    assert_eq!(b.value("Type", "Furniture", "AssetType").as_deref(), Some("Moveable"));
    assert_eq!(b.value("Type", "FCU-1", "AssetType").as_deref(), Some("Fixed"));
    // Components: unique names, their rooms (stated, found by where they are, or both sides of a door).
    let comps = col(&b, "Component", "Name");
    assert!(comps.contains(&"Light".to_string()) && comps.contains(&"Light (2)".to_string()), "{comps:?}");
    assert_eq!(b.value("Component", "FCU-101", "Space").as_deref(), Some("101"), "found inside 101");
    assert_eq!(b.value("Component", "Duct 101", "Space").as_deref(), Some("101"), "in a system, and stated");
    assert_eq!(b.value("Component", "Light (2)", "Space").as_deref(), Some("102"));
    assert_eq!(b.value("Component", "Bed 201", "Space").as_deref(), Some("201"));
    assert_eq!(b.value("Component", "D101", "Space").as_deref(), Some("101,102"), "between two rooms");
    assert_eq!(b.value("Component", "FCU-101", "ExtIdentifier").as_deref(), Some(FCU));
    assert!(!comps.iter().any(|c| c.starts_with("Wall") || c.starts_with("Slab")));
    // Systems and their components.
    assert_eq!(col(&b, "System", "Name"), ["Chilled Water", "Supply Air"]);
    assert_eq!(b.value("System", "Supply Air", "ComponentNames").as_deref(), Some("FCU-101,Duct 101"));
    // Attributes: property sets, not quantities.
    let attrs = b.sheet("Attribute").unwrap();
    let fire: Vec<&Vec<String>> = attrs.iter().filter(|r| r.first().map(String::as_str) == Some("FireRating")).collect();
    assert_eq!(fire.len(), 2);
    assert!(fire.iter().any(|r| r.get(5).map(String::as_str) == Some("D102") && r.get(6).map(String::as_str) == Some("45 MIN")));
    assert!(!col(&b, "Attribute", "Name").iter().any(|n| n == "GrossFloorArea"));
    // Coordinates: each space's and component's box, in metres.
    let ll: Vec<Vec<String>> = b.sheet("Coordinate").unwrap().iter().filter(|r| r.get(5).map(String::as_str) == Some("FCU-101")).cloned().collect();
    assert_eq!(ll.len(), 2);
    assert_eq!(ll[0][6..9], ["0.5".to_string(), "0.75".into(), "2.4".into()]);
    assert_eq!(ll[1][6..9], ["1.5".to_string(), "1.25".into(), "2.7".into()]);
    // The BCF issue: who, about what, owner and what to do.
    let issue = &b.sheet("Issue").unwrap()[1];
    assert_eq!(issue[0], "FCU access panel");
    assert_eq!(issue[1], "arch@example.com");
    assert_eq!((issue[4].as_str(), issue[7].as_str(), issue[8].as_str()), ("High", "Component", "FCU-101"));
    assert_eq!((issue[12].as_str(), issue[13].as_str()), ("mech@example.com", "Keep 600 mm clear below the unit."));
    // Our validator agrees from the file; no blank cells anywhere.
    let v = s.execute("cobie.validate", &json!({"path": file.display().to_string()})).unwrap();
    assert_eq!((v["ok"].as_bool(), v["errors"].as_u64(), v["warnings"].as_u64()), (Some(true), Some(0), Some(0)), "{v}");
    for (n, rows) in &b.sheets {
        for (r, row) in rows.iter().enumerate().skip(1) {
            let cols = cobie::SHEETS.iter().find(|(s, _)| s == n).unwrap().1.len();
            assert_eq!(row.len(), cols, "{n} row {r}");
            assert!(row.iter().all(|c| !c.is_empty()), "{n} row {r}: {row:?}");
        }
    }
}

#[test]
fn the_validator_catches_what_a_broken_workbook_gets_wrong() {
    let (mut s, d) = hotel("broken");
    let (_, good) = export(&mut s, &d.join("good.xlsx"));
    assert!(cobie::validate(&good).iter().all(|p| !p.error));
    let set = |b: &mut Book, sheet: &str, row: usize, column: &str, v: &str| {
        let rows = b.sheet_mut(sheet).unwrap();
        let i = rows[0].iter().position(|c| c == column).unwrap();
        rows[row - 1][i] = v.to_string();
    };
    // (what to break, the sheet, Excel row and column the validator must name, its rule)
    type Break = fn(&mut Book, &dyn Fn(&mut Book, &str, usize, &str, &str));
    let cases: Vec<(Break, &str, usize, &str, &str)> = vec![
        (|b, set| set(b, "Space", 2, "FloorName", "Level 9"), "Space", 2, "FloorName", "reference"),
        (|b, set| set(b, "Component", 2, "TypeName", "No Such Type"), "Component", 2, "TypeName", "reference"),
        (|b, set| set(b, "Component", 3, "Space", "101,999"), "Component", 3, "Space", "reference"),
        (|b, set| set(b, "Floor", 3, "Name", "Level 1"), "Floor", 3, "Name", "unique"),
        (|b, set| set(b, "Type", 2, "CreatedOn", "21/10/2026"), "Type", 2, "CreatedOn", "date"),
        (|b, set| set(b, "Space", 3, "CreatedBy", "someone@example.com"), "Space", 3, "CreatedBy", "contact"),
        (|b, set| set(b, "Component", 2, "Description", ""), "Component", 2, "Description", "required"),
        (|b, set| set(b, "Floor", 2, "Elevation", "ground"), "Floor", 2, "Elevation", "number"),
        (|b, set| set(b, "Floor", 2, "Category", "Storey"), "Floor", 2, "Category", "pick list"),
        (|b, set| set(b, "Contact", 2, "Email", "not-an-email"), "Contact", 2, "Email", "email"),
        (|b, set| set(b, "Attribute", 2, "RowName", "Nowhere"), "Attribute", 2, "RowName", "reference"),
        (|b, set| set(b, "System", 2, "ComponentNames", "FCU-101,Ghost"), "System", 2, "ComponentNames", "reference"),
        (|b, set| set(b, "Space", 1, "FloorName", "Floor"), "Space", 1, "FloorName", "header"),
        (|b, _| b.sheets.retain(|(n, _)| n != "Type"), "Type", 0, "", "sheet"),
        (
            |b, _| {
                let r = b.sheet("Facility").unwrap()[1].clone();
                b.sheet_mut("Facility").unwrap().push(r);
            },
            "Facility",
            0,
            "Name",
            "one facility",
        ),
    ];
    for (k, (brk, sheet, row, column, rule)) in cases.into_iter().enumerate() {
        let mut b = good.clone();
        brk(&mut b, &set);
        let problems = cobie::validate(&b);
        assert!(
            problems.iter().any(|p| p.error && p.sheet == sheet && p.row == row && p.column == column && p.rule == rule),
            "case {k} ({sheet} {row} {column} {rule}): {problems:?}"
        );
        // And through a file, as `cobie.validate` reads one.
        if k < 3 {
            let path = d.join(format!("broken-{k}.xlsx"));
            std::fs::write(&path, b.write().unwrap()).unwrap();
            let v = s.execute("cobie.validate", &json!({"path": path.display().to_string()})).unwrap();
            assert_eq!(v["ok"], false, "{v}");
            assert!(v["problems"].as_array().unwrap().iter().any(|p| p["sheet"] == sheet && p["row"] == row && p["rule"] == rule), "{v}");
        }
    }
    // A component with no room is a warning, not an error.
    let mut b = good.clone();
    set(&mut b, "Component", 2, "Space", "n/a");
    let p = cobie::validate(&b);
    assert!(p.iter().all(|p| !p.error) && p.iter().any(|p| p.rule == "reference" && p.sheet == "Component"), "{p:?}");
}

#[test]
fn room_door_and_equipment_schedules() {
    let (mut s, d) = hotel("schedules");
    let file = d.join("schedules.xlsx");
    let r = s.execute("handover.schedules", &json!({"path": file.display().to_string()})).unwrap();
    assert_eq!(r["rows"], json!({"Rooms": 3, "Doors": 2, "Equipment": 6}), "{r}");
    let b = Book::read(&std::fs::read(&file).unwrap()).unwrap();
    let rooms = b.sheet("Rooms").unwrap();
    assert_eq!(rooms[0], ["Room", "Storey", "Area (m²)", "Height (m)", "Doors", "Equipment", "GlobalId"]);
    assert_eq!(rooms[1][..6], ["101", "Level 1", "20.25", "3.00", "1", "2"].map(String::from));
    assert_eq!(rooms[2][..6], ["102", "Level 1", "19.50", "3.00", "2", "3"].map(String::from));
    assert_eq!(rooms[3][..6], ["201", "Level 2", "20.25", "3.00", "0", "1"].map(String::from));
    let doors = b.sheet("Doors").unwrap();
    assert_eq!(doors[1][..7], ["D101", "Door", "Level 1", "101 / 102", "0.90", "2.10", "20 MIN"].map(String::from));
    assert_eq!(doors[2][3..7], ["102", "0.90", "2.10", "45 MIN"].map(String::from));
    let eq = b.sheet("Equipment").unwrap();
    let fcu = eq.iter().find(|r| r[0] == "FCU-101").unwrap();
    assert_eq!(fcu[1..8], ["UnitaryEquipment", "FCU-1", "Acme Air", "FC-400", "Chilled Water, Supply Air", "101", "Level 1"].map(String::from));
    // One schedule only; a wrong name is refused.
    let r = s.execute("handover.schedules", &json!({"path": file.display().to_string(), "schedules": ["doors"]})).unwrap();
    assert_eq!(r["rows"], json!({"Doors": 2}));
    assert!(s.execute("handover.schedules", &json!({"path": file.display().to_string(), "schedules": "rooms,windows"})).is_err());
}

/// A two-room plan with a doorway, built by WALLGEN, as `ifc_import` makes it.
fn built() -> Session {
    use cadaddy_engine::doc::{Drawing, Entity, EntityKind, Handle, Line};
    use cadaddy_engine::geom::Vec3;
    let mut d = Drawing::new_imperial();
    let mut line = |a: (f64, f64), b: (f64, f64)| {
        let p = |(x, y): (f64, f64)| Vec3 { x: x * 12.0, y: y * 12.0, z: 0.0 };
        let h = Handle(d.handseed.max(0x100));
        d.handseed = h.0 + 1;
        let mut e = Entity::new(h, EntityKind::Line(Line { a: p(a), b: p(b) }));
        e.common.layer = "A-WALL".into();
        d.model.push(e);
    };
    for (a, b) in [((0., 0.), (40., 0.)), ((40., 0.), (40., 10.)), ((40., 10.), (0., 10.)), ((0., 10.), (0., 0.))] {
        line(a, b);
    }
    for (a, b) in [((0.5, 0.5), (39.5, 0.5)), ((39.5, 0.5), (39.5, 9.5)), ((39.5, 9.5), (0.5, 9.5)), ((0.5, 9.5), (0.5, 0.5))] {
        line(a, b);
    }
    for x in [20.0, 20.5] {
        line((x, 0.5), (x, 3.0));
        line((x, 6.0), (x, 9.5));
    }
    let mut s = Session::empty();
    s.open_drawing(d, "Two Rooms", None);
    let rules = json!({
        "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 9.5, "ceiling_ft": 8, "base": [0, 0]}],
        "layers": [{"layer": "A-WALL", "kind": "wall", "skip_curves": true}]
    });
    s.execute("wallgen", &json!({ "rules": rules })).unwrap();
    s
}

#[test]
fn a_model_cadaddy_built_hands_over_through_its_ifc_export() {
    hooks();
    let d = dir("built");
    let mut s = built();
    let file = d.join("built-cobie.xlsx");
    let r = s.execute("cobie.export", &json!({"path": file.display().to_string(), "created_on": ON})).unwrap();
    assert!(r["source"].as_str().unwrap().starts_with("the built model"), "{r}");
    assert_eq!(r["validation"]["errors"], 0, "{r}");
    let b = Book::read(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(col(&b, "Floor", "Name"), ["L1"]);
    assert_eq!(col(&b, "Contact", "Email"), ["handover@example.com"]);
    assert_eq!(col(&b, "Facility", "Name").len(), 1);
    let v = s.execute("cobie.validate", &json!({"path": file.display().to_string()})).unwrap();
    assert_eq!(v["errors"], 0, "{v}");
    // The drawing is left as it was: the IFC round trip is a copy.
    assert!(cadaddy_engine::cmd::ifcmodel::elements(s.doc().unwrap()).is_empty());
}

#[test]
fn bad_requests_are_refused_without_panicking() {
    let (mut s, d) = hotel("bad");
    let out = d.join("x.xlsx").display().to_string();
    for (cmd, p) in [
        ("cobie.export", json!({})),
        ("cobie.export", json!({"path": out, "author": "not an email"})),
        ("cobie.export", json!({"path": out, "created_on": "yesterday"})),
        ("cobie.export", json!({"path": out, "zones": "all"})),
        ("cobie.validate", json!({})),
        ("cobie.validate", json!({"path": d.join("missing.xlsx").display().to_string()})),
        ("cobie.validate", json!({"data": "%%%"})),
        ("cobie.validate", json!({"data": cadaddy_engine::cmd::file::base64_encode(b"PK not really")})),
        ("handover.schedules", json!({})),
        ("handover.schedules", json!({"path": out, "schedules": 7})),
    ] {
        let r = s.execute(cmd, &p);
        // Zones that aren't a list are ignored, not refused.
        if p.get("zones").is_some() {
            assert!(r.is_ok(), "{cmd} {p}");
        } else {
            assert!(r.is_err(), "{cmd} {p}");
        }
    }
    // Zones, as told.
    let r = s.execute("cobie.export", &json!({"path": out, "created_on": ON, "zones": [{"name": "Guest rooms", "category": "Occupancy", "spaces": ["101", "102"]}, {"name": "Ghost", "spaces": ["999"]}]})).unwrap();
    assert_eq!(r["validation"]["errors"], 1, "the ghost zone's room isn't a space: {r}");
    // An empty drawing has nothing to hand over.
    let mut e = Session::empty();
    e.new_drawing(false);
    let err = e.execute("cobie.export", &json!({"path": out})).unwrap_err().to_string();
    assert!(err.contains("nothing to hand over"), "{err}");
}
