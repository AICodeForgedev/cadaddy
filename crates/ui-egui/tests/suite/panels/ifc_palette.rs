//! The IFC Model palette on a made-up model: the tree, the selected duct's properties and a BCF
//! issue raised on it, in light and dark. `CADADDY_IFC_SHOTS=<dir>` saves the renders (to look at).

use cadaddy_ui_egui::{CadApp, Services, theme};
use egui_kittest::Harness;
use serde_json::json;

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

fn app() -> CadApp {
    let d = cadaddy_io::read(hotel_ifc().as_bytes(), "hotel.ifc").unwrap();
    let mut app = CadApp::new(cadaddy_engine::Session::empty(), Services::default());
    app.session.open_drawing(d, "hotel", None);
    let duct = app.run("ifc.properties", json!({ "guid": DUCT })).unwrap();
    app.session.set_selection(vec![cadaddy_engine::doc::Handle::parse_hex(duct["handle"].as_str().unwrap()).unwrap()]);
    app.run("bcf.create", json!({ "title": "Duct below the door head?", "comment": "Check the bottom of duct 3 against the door head.", "assignee": "mech@example.com", "snapshot": false })).unwrap();
    let _ = (WALL_N, WALL_W, WALL_2, COLUMN, DOOR);
    app.run("ifcmodel", serde_json::Value::Null).unwrap();
    app
}

#[test]
fn the_ifc_palette_renders_light_and_dark() {
    for (name, dark) in [("ifc-palette-light.png", false), ("ifc-palette-dark.png", true)] {
        let mut h = Harness::builder().with_size(egui::vec2(560.0, 900.0)).wgpu().build_ui_state(
            move |ui, app: &mut CadApp| {
                theme::apply(ui.ctx(), if dark { egui::Theme::Dark } else { egui::Theme::Light }, false);
                cadaddy_ui_egui::dialogs::show(app, ui.ctx());
            },
            app(),
        );
        h.run_steps(4);
        assert_eq!(h.state().ui.dialog.as_deref(), Some(cadaddy_ui_egui::ifcpalette::DIALOG), "the palette stays open");
        if let Some(dir) = std::env::var_os("CADADDY_IFC_SHOTS") {
            let dir = std::path::PathBuf::from(dir);
            std::fs::create_dir_all(&dir).unwrap();
            h.render().unwrap().save(dir.join(name)).unwrap();
        }
    }
}
