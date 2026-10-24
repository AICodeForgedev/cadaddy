//! A made-up IFC4 project to share (Q350): a small hotel wing in metres, `floors` storeys of
//! `rooms` guest rooms off a corridor — spaces, walls, doors, columns, slabs, a supply duct run
//! in its system and a light in every room — with names planted where real files keep them
//! (the project, site and building, a person, an organisation and its address in the owner
//! history, the header's author, a manager's office, an installer's comment, a manufacturer).
//! Every planted name is in [`PLANTED`]: the shared page must carry none of them.

use std::fmt::Write as _;

/// The made-up names the fixture plants (none is a real person, firm or place).
pub const PLANTED: [&str; 8] = [
    "Lakeside Example Inn",
    "Marigold",
    "Quillonby",
    "Fernhollow Design Group",
    "Bramblewood Lane",
    "Zephyrine Hollow",
    "marigold@fernhollow.example",
    "Thistledown Lighting Works",
];

struct Step {
    lines: Vec<String>,
    ids: usize,
}

impl Step {
    /// The next GlobalId-shaped id (22 characters) from a kind letter.
    fn gid(&mut self, kind: char) -> String {
        self.ids += 1;
        format!("{kind}{:0>21}", self.ids)
    }

    /// A product (an object with a box body) of `entity`, its attributes after the shape `tail`.
    fn product(&mut self, entity: &str, k: char, name: &str, at: [f64; 3], size: [f64; 3], tail: &str) -> usize {
        let (place, pds) = self.boxed(at, size);
        let g = self.gid(k);
        self.add(format!("{entity}('{g}',$,'{name}',$,$,#{place},#{pds},$,{tail})"))
    }

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
}

/// The IFC text: `floors` (1..=12) storeys of `rooms` (1..=60) rooms, half each side of a corridor.
pub fn share_project_ifc(floors: usize, rooms: usize) -> String {
    let (floors, rooms) = (floors.clamp(1, 12), rooms.clamp(1, 60));
    let mut s = Step { lines: Vec::new(), ids: 0 };
    // Who made it: planted.
    let person = s.add("IFCPERSON($,'Quillonby','Marigold',$,$,$,$,$)");
    let addr = s.add("IFCPOSTALADDRESS(.OFFICE.,$,$,$,('4821 Bramblewood Lane'),$,'Zephyrine Hollow',$,'00000',$)");
    let mail = s.add("IFCTELECOMADDRESS(.OFFICE.,$,$,$,$,('marigold@fernhollow.example'),$,$,$)");
    let org = s.add(format!("IFCORGANIZATION($,'Fernhollow Design Group LLC',$,$,(#{addr},#{mail}))"));
    let po = s.add(format!("IFCPERSONANDORGANIZATION(#{person},#{org},$)"));
    let app_org = s.add("IFCORGANIZATION($,'CADaddy',$,$,$)");
    let app = s.add(format!("IFCAPPLICATION(#{app_org},'1','CADaddy','CADaddy')"));
    let owner = s.add(format!("IFCOWNERHISTORY(#{po},#{app},$,.ADDED.,$,$,$,0)"));
    let unit = s.add("IFCSIUNIT(*,.LENGTHUNIT.,$,.METRE.)");
    let units = s.add(format!("IFCUNITASSIGNMENT((#{unit}))"));
    let id = s.gid('P');
    let project = s.add(format!("IFCPROJECT('{id}',#{owner},'Lakeside Example Inn',$,$,$,$,$,#{units})"));
    let id = s.gid('S');
    let site = s.add(format!("IFCSITE('{id}',#{owner},'Zephyrine Hollow Parcel',$,$,$,$,$,.ELEMENT.,$,$,$,$,#{addr})"));
    let id = s.gid('B');
    let bldg = s.add(format!("IFCBUILDING('{id}',#{owner},'Lakeside Example Inn',$,$,$,$,$,.ELEMENT.,$,$,#{addr})"));
    let id = s.gid('R');
    s.add(format!("IFCRELAGGREGATES('{id}',$,$,$,#{project},(#{site}))"));
    let id = s.gid('R');
    s.add(format!("IFCRELAGGREGATES('{id}',$,$,$,#{site},(#{bldg}))"));
    let height = 3.2;
    let half = rooms.div_ceil(2);
    let (room_w, room_d, corridor) = (4.0, 8.0, 2.0);
    let length = half as f64 * room_w;
    let id = s.gid('T');
    let duct_type = s.add(format!("IFCDUCTSEGMENTTYPE('{id}',$,'Duct 400x300',$,$,$,$,$,$,.RIGIDSEGMENT.)"));
    let id = s.gid('T');
    let light_type = s.add(format!("IFCLIGHTFIXTURETYPE('{id}',$,'Downlight A',$,$,$,$,$,$,.POINTSOURCE.)"));
    let id = s.gid('Y');
    let system = s.add(format!("IFCDISTRIBUTIONSYSTEM('{id}',$,'Supply Air',$,$,$,.SUPPLYAIR.)"));
    let mut storeys = Vec::new();
    let (mut ducts, mut lights) = (Vec::new(), Vec::new());
    for f in 0..floors {
        let z = f as f64 * height;
        let id = s.gid('L');
        let storey = s.add(format!("IFCBUILDINGSTOREY('{id}',$,'Level {}',$,$,$,$,$,.ELEMENT.,{z:.2})", f + 1));
        storeys.push(storey);
        let mut contained = Vec::new();
        let mut spaces = Vec::new();
        contained.push(s.product("IFCSLAB", 'F', &format!("Slab L{}", f + 1), [length * 0.5, 0.0, z - 0.2], [length, room_d * 2.0 + corridor, 0.2], ".FLOOR."));
        for side in [-1.0, 1.0] {
            contained.push(s.product("IFCWALL", 'W', "Corridor wall", [length * 0.5, side * corridor * 0.5, z], [length, 0.15, height], ".STANDARD."));
            contained.push(s.product("IFCWALL", 'W', "Exterior wall", [length * 0.5, side * (corridor * 0.5 + room_d), z], [length, 0.3, height], ".STANDARD."));
        }
        for r in 0..rooms {
            let side = if r < half { -1.0 } else { 1.0 };
            let i = r % half;
            let x = (i as f64 + 0.5) * room_w;
            let y = side * (corridor + room_d) * 0.5;
            let number = format!("{}{:02}", f + 1, r + 1);
            let space = s.product("IFCSPACE", 'R', &number, [x, y, z], [room_w - 0.2, room_d - 0.2, height - 0.3], ".ELEMENT.,.INTERNAL.,$");
            spaces.push(space);
            let long = if f == 0 && r == 0 { "Manager office - Marigold Quillonby".to_string() } else { "Guest Room".to_string() };
            let lp = s.add(format!("IFCPROPERTYSINGLEVALUE('LongName',$,IFCLABEL('{long}'),$)"));
            let id = s.gid('Q');
            let ps = s.add(format!("IFCPROPERTYSET('{id}',$,'Pset_SpaceCommon',$,(#{lp}))"));
            let id = s.gid('R');
            s.add(format!("IFCRELDEFINESBYPROPERTIES('{id}',$,$,$,(#{space}),#{ps})"));
            contained.push(s.product("IFCWALL", 'W', "Partition", [i as f64 * room_w, y, z], [0.12, room_d, height], ".STANDARD."));
            contained.push(s.product("IFCDOOR",
                'D',
                &format!("Door {number}"),
                [x, side * corridor * 0.5, z],
                [0.9, 0.2, 2.1],
                "2.1,0.9,.DOOR.,$,$",
            ));
            let light = s.product("IFCLIGHTFIXTURE", 'E', &format!("Light {number}"), [x, y, z + height - 0.4], [0.3, 0.3, 0.1], ".POINTSOURCE.");
            let a = s.add("IFCPROPERTYSINGLEVALUE('Manufacturer',$,IFCLABEL('Thistledown Lighting Works'),$)");
            let b = s.add("IFCPROPERTYSINGLEVALUE('ModelLabel',$,IFCLABEL('DL-12'),$)");
            let id = s.gid('Q');
            let ps = s.add(format!("IFCPROPERTYSET('{id}',$,'Pset_ManufacturerTypeInformation',$,(#{a},#{b}))"));
            let id = s.gid('R');
            s.add(format!("IFCRELDEFINESBYPROPERTIES('{id}',$,$,$,(#{light}),#{ps})"));
            lights.push(light);
            contained.push(light);
        }
        for c in 0..=half {
            contained.push(s.product("IFCCOLUMN", 'C', &format!("Column {}", c + 1), [c as f64 * room_w, 0.0, z], [0.4, 0.4, height], ".COLUMN."));
        }
        let duct = s.product("IFCDUCTSEGMENT", 'M', &format!("Supply Duct L{}", f + 1), [length * 0.5, 0.0, z + height - 0.6], [length, 0.4, 0.3], ".RIGIDSEGMENT.");
        let a = s.add("IFCPROPERTYSINGLEVALUE('Status',$,IFCLABEL('New'),$)");
        let b = s.add("IFCPROPERTYSINGLEVALUE('Comments',$,IFCTEXT('Installed by Fernhollow Design Group LLC, call 555-010-0142'),$)");
        let id = s.gid('Q');
        let ps = s.add(format!("IFCPROPERTYSET('{id}',$,'Pset_DuctSegmentCommon',$,(#{a},#{b}))"));
        let id = s.gid('R');
        s.add(format!("IFCRELDEFINESBYPROPERTIES('{id}',$,$,$,(#{duct}),#{ps})"));
        ducts.push(duct);
        contained.push(duct);
        let list = |v: &[usize]| v.iter().map(|x| format!("#{x}")).collect::<Vec<_>>().join(",");
        let id = s.gid('R');
        s.add(format!("IFCRELCONTAINEDINSPATIALSTRUCTURE('{id}',$,$,$,({}),#{storey})", list(&contained)));
        let id = s.gid('R');
        s.add(format!("IFCRELAGGREGATES('{id}',$,$,$,#{storey},({}))", list(&spaces)));
    }
    let list = |v: &[usize]| v.iter().map(|x| format!("#{x}")).collect::<Vec<_>>().join(",");
    let id = s.gid('R');
    s.add(format!("IFCRELAGGREGATES('{id}',$,$,$,#{bldg},({}))", list(&storeys)));
    let id = s.gid('R');
    s.add(format!("IFCRELDEFINESBYTYPE('{id}',$,$,$,({}),#{duct_type})", list(&ducts)));
    let id = s.gid('R');
    s.add(format!("IFCRELDEFINESBYTYPE('{id}',$,$,$,({}),#{light_type})", list(&lights)));
    let id = s.gid('R');
    s.add(format!("IFCRELASSIGNSTOGROUP('{id}',$,$,$,({}),$,#{system})", list(&ducts)));
    let mut t = String::from(
        "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('ViewDefinition [ReferenceView]'),'2;1');\n\
         FILE_NAME('Lakeside Example Inn.ifc','2026-10-22T00:00:00',('Marigold Quillonby'),('Fernhollow Design Group LLC'),'','CADaddy','');\n\
         FILE_SCHEMA(('IFC4'));\nENDSEC;\nDATA;\n",
    );
    for (i, l) in s.lines.iter().enumerate() {
        let _ = writeln!(t, "#{}={l};", i + 1);
    }
    t + "ENDSEC;\nEND-ISO-10303-21;\n"
}
