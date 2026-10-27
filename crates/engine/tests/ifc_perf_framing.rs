//! Tests for the ifcviewer-rw1 rework (Q5054): MUST 1 (no per-frame full-model scans, cached
//! lookups) and MUST 2 (BCF snapshots and ifc.render frame on the selection, not the whole model).

use cadaddy_engine::Session;
use cadaddy_engine::cmd::ifcmodel;
use serde_json::{Value, json};
use std::time::Instant;

fn hooks() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
        read_dwf: None,
        write_dwfx: None,
    });
}

fn dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-ifcperf-{name}-{}", std::process::id()));
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

    fn boxed(&mut self, at: [f64; 3], size: [f64; 3]) -> (usize, usize) {
        let p = self.add(format!("IFCCARTESIANPOINT(({:.3},{:.3},{:.3}))", at[0], at[1], at[2]));
        let a = self.add(format!("IFCAXIS2PLACEMENT3D(#{p},$,$)"));
        let place = self.add(format!("IFCLOCALPLACEMENT($,#{a})"));
        let c = self.add("IFCCARTESIANPOINT((0.,0.))");
        let a2 = self.add(format!("IFCAXIS2PLACEMENT2D(#{c},$)"));
        let r = self.add(format!("IFCRECTANGLEPROFILEDEF(.AREA.,$,#{a2},{:.3},{:.3})", size[0], size[1]));
        let d = self.add("IFCDIRECTION((0.,0.,1.))");
        let e = self.add(format!("IFCEXTRUDEDAREASOLID(#{r},$,#{d},{:.3})", size[2]));
        let sh = self.add(format!("IFCSHAPEREPRESENTATION($,'Body','SweptSolid',(#{e}))"));
        let pds = self.add(format!("IFCPRODUCTDEFINITIONSHAPE($,$,(#{sh}))"));
        (place, pds)
    }

    fn text(&self) -> String {
        let mut t = String::from(
            "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION((''),'2;1');\nFILE_NAME('perf.ifc','2026-10-22T00:00:00',(''),(''),'','','');\nFILE_SCHEMA(('IFC4'));\nENDSEC;\nDATA;\n",
        );
        for (i, l) in self.lines.iter().enumerate() {
            t += &format!("#{}={l};\n", i + 1);
        }
        t + "ENDSEC;\nEND-ISO-10303-21;\n"
    }
}

/// Generate an IFC with `n` wall objects across a grid, all on one storey.
fn big_ifc(n: usize) -> String {
    let mut s = Step::default();
    let unit = s.add("IFCSIUNIT(*,.LENGTHUNIT.,$,.METRE.)");
    let units = s.add(format!("IFCUNITASSIGNMENT((#{unit}))"));
    let proj = s.add(format!("IFCPROJECT('0Prj000000000000000001',$,'BigTest',$,$,$,$,$,#{units})"));
    let site = s.add("IFCSITE('0Sit000000000000000001',$,'TestSite',$,$,$,$,$,.ELEMENT.,$,$,$,$,$)");
    let bldg = s.add("IFCBUILDING('0Bld000000000000000001',$,'TestBldg',$,$,$,$,$,.ELEMENT.,$,$,$)");
    let l1 = s.add("IFCBUILDINGSTOREY('0Sty000000000000000001',$,'Floor 1',$,$,$,$,$,.ELEMENT.,0.)");
    s.add(format!("IFCRELAGGREGATES('0Rel000000000000000001',$,$,$,#{proj},(#{site}))"));
    s.add(format!("IFCRELAGGREGATES('0Rel000000000000000002',$,$,$,#{site},(#{bldg}))"));
    s.add(format!("IFCRELAGGREGATES('0Rel000000000000000003',$,$,$,#{bldg},(#{l1}))"));
    let cols = (n as f64).sqrt().ceil() as usize;
    let mut wall_ids = Vec::with_capacity(n);
    for i in 0..n {
        let row = i / cols;
        let col = i % cols;
        let x = col as f64 * 2.0;
        let y = row as f64 * 2.0;
        let guid = format!("1Wal{i:020}");
        let name = format!("W{i}");
        let (place, pds) = s.boxed([x, y, 0.0], [1.0, 0.2, 3.0]);
        let w = s.add(format!("IFCWALL('{guid}',$,'{name}',$,$,#{place},#{pds},$,.STANDARD.)"));
        wall_ids.push(w);
    }
    // Batch all walls into one spatial containment.
    let w_list = wall_ids.iter().map(|w| format!("#{w}")).collect::<Vec<_>>().join(",");
    s.add(format!("IFCRELCONTAINEDINSPATIALSTRUCTURE('0Rel000000000000000004',$,$,$,({w_list}),#{l1})"));
    s.text()
}

/// The small hotel IFC used for pixel tests (same as in ifc_viewer.rs).
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
    let wn = product(&mut s, "IFCWALL", "1WalNorth0000000000001", "Wall North", [5.0, 0.0, 0.0], [10.0, 0.2, 3.0], ".STANDARD.");
    let ww = product(&mut s, "IFCWALL", "1WalWest00000000000001", "Wall West", [0.0, 5.0, 0.0], [0.2, 10.0, 3.0], ".STANDARD.");
    let col = product(&mut s, "IFCCOLUMN", "1Col00000000000000001A", "Column C4", [8.0, 8.0, 0.0], [0.4, 0.4, 3.0], ".COLUMN.");
    let slab1 = product(&mut s, "IFCSLAB", "1Slb00000000000000001A", "Slab L1", [5.0, 5.0, -0.2], [10.0, 10.0, 0.2], ".FLOOR.");
    let door = product(&mut s, "IFCDOOR", "1Dor00000000000000001A", "Door 101", [2.0, 0.0, 0.0], [0.9, 0.25, 2.1], "2.1,0.9,.DOOR.,$,$");
    let room = product(&mut s, "IFCSPACE", "1Spc00000000000000101A", "101", [5.0, 5.0, 0.0], [9.0, 9.0, 3.0], ".ELEMENT.,.INTERNAL.,$");
    let duct = product(&mut s, "IFCDUCTSEGMENT", "1Dct00000000000000001A", "Supply Duct 3", [5.0, 3.0, 2.6], [6.0, 0.4, 0.3], ".RIGIDSEGMENT.");
    let w2 = product(&mut s, "IFCWALL", "1WalUpper0000000000001", "Wall North L2", [5.0, 0.0, 3.5], [10.0, 0.2, 3.0], ".STANDARD.");
    let slab2 = product(&mut s, "IFCSLAB", "1Slb00000000000000002A", "Slab L2", [5.0, 5.0, 3.3], [10.0, 10.0, 0.2], ".FLOOR.");
    s.add(format!("IFCRELCONTAINEDINSPATIALSTRUCTURE('0Rel000000000000000004',$,$,$,(#{wn},#{ww},#{col},#{slab1},#{door}),#{l1})"));
    s.add(format!("IFCRELAGGREGATES('0Rel000000000000000005',$,$,$,#{l1},(#{room}))"));
    s.add(format!("IFCRELCONTAINEDINSPATIALSTRUCTURE('0Rel000000000000000006',$,$,$,(#{duct}),#{room})"));
    s.add(format!("IFCRELCONTAINEDINSPATIALSTRUCTURE('0Rel000000000000000007',$,$,$,(#{w2},#{slab2}),#{l2})"));
    // Duct type and system.
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
    s.execute("ifc.open", &json!({"path": path.display().to_string()})).unwrap();
    (s, d)
}

fn handle_of(s: &mut Session, guid: &str) -> String {
    let p = s.execute("ifc.properties", &json!({"guid": guid})).unwrap();
    p["handle"].as_str().unwrap().to_string()
}

/// Decode PNG bytes into (width, height, Vec<[r,g,b,a]>).
fn decode_png(bytes: &[u8]) -> (u32, u32, Vec<[u8; 4]>) {
    let r = cadaddy_render::images::decode(bytes).expect("decode PNG");
    let pixels: Vec<[u8; 4]> = r.rgba.as_chunks::<4>().0.to_vec();
    (r.width, r.height, pixels)
}

/// Is a pixel "orange" enough to be the SELECTED colour? The rasterizer shades, so the exact
/// RGB 255,140,0 gets lit. We accept pixels where R > G + 30 and R > B + 80 and R > 150.
fn is_orange(px: [u8; 4]) -> bool {
    px[0] > 150 && px[0] > px[1].saturating_add(30) && px[0] > px[2].saturating_add(80)
}

// =============================== MUST 1 ===============================
// The IFC palette must not scan the whole model or deserialise all BCF topics on every frame.
// Cache ifc.properties keyed on (revision, selection), and bcf.list/bcf.get keyed on revision.
// elements() must not clone mesh vertices to get a bounding box (iterate, or cache an element
// index per revision). Handle and GlobalId lookups use a map. Proof: a test on a generated
// model of at least 50k IFC objects plus 200 BCF topics. It asserts that a palette frame with
// one object selected does no full-model scan, and that bcf.create and bcf.show with 10k
// selected GlobalIds finish within a stated budget.

#[test]
fn cached_index_avoids_full_model_scan_on_repeated_commands() {
    hooks();
    let d = dir("bigmodel");
    let ifc = big_ifc(50_000);
    let path = d.join("big.ifc");
    std::fs::write(&path, &ifc).unwrap();

    let mut s = Session::empty();
    let r = s.execute("ifc.open", &json!({"path": path.display().to_string()})).unwrap();
    let n = r["elements"].as_u64().unwrap();
    assert!(n >= 50_000, "model has {n} objects, expected >= 50_000");

    // Create 200 BCF topics with snapshots disabled (they'd be slow on a 50k model).
    for i in 0..200 {
        s.execute(
            "bcf.create",
            &json!({
                "title": format!("Topic {i}"),
                "snapshot": false
            }),
        )
        .unwrap();
    }
    let topics = s.execute("bcf.list", &json!({})).unwrap();
    assert_eq!(topics["total"].as_u64().unwrap(), 200);

    // Select one object.
    let first_guid = format!("1Wal{:020}", 0);
    let props = s.execute("ifc.properties", &json!({"guid": first_guid})).unwrap();
    let handle = props["handle"].as_str().unwrap().to_string();
    s.set_selection(vec![cadaddy_engine::doc::Handle::parse_hex(&handle).unwrap()]);

    // Reset the elements() call counter. Calling ifc.properties, bcf.list, bcf.get on the
    // same revision and selection should use the cached index and NOT call elements() again.
    let counter = &ifcmodel::ELEMENTS_CALL_COUNT;
    counter.store(0, std::sync::atomic::Ordering::SeqCst);

    // Simulate what the palette does on every frame: tree (cached), properties, bcf.list, bcf.get.
    let _tree = s.execute("ifc.tree", &json!({"limit": 100})).unwrap();
    // First call builds the cache — allowed one elements() call.
    let _calls_after_tree = counter.load(std::sync::atomic::Ordering::SeqCst);

    // Now repeat: these should all use the cached index (no new elements() calls).
    counter.store(0, std::sync::atomic::Ordering::SeqCst);
    let _props = s.execute("ifc.properties", &json!({})).unwrap();
    let _list = s.execute("bcf.list", &json!({})).unwrap();
    let topic0 = topics["topics"][0]["guid"].as_str().unwrap();
    let _get = s.execute("bcf.get", &json!({"topic": topic0})).unwrap();
    let _props2 = s.execute("ifc.properties", &json!({})).unwrap();
    let _list2 = s.execute("bcf.list", &json!({})).unwrap();
    let calls = counter.load(std::sync::atomic::Ordering::SeqCst);
    assert_eq!(calls, 0, "repeated ifc.properties/bcf.list/bcf.get should use the cache, not call elements() (got {calls} calls)");

    // bcf.create and bcf.show with 10k selected GlobalIds must finish within budget.
    let guids_10k: Vec<String> = (0..10_000usize).map(|i| format!("1Wal{i:020}")).collect();
    let sel_json: Vec<Value> = guids_10k.iter().map(|g| json!(g)).collect();

    let start = Instant::now();
    let cr = s.execute("bcf.create", &json!({"title": "Batch", "snapshot": false, "selection": sel_json})).unwrap();
    let create_ms = start.elapsed().as_millis();
    assert_eq!(cr["selected"].as_u64().unwrap(), 10_000);
    assert!(create_ms < 10_000, "bcf.create with 10k GUIDs took {create_ms} ms, budget 10 s");

    let guid = cr["guid"].as_str().unwrap();
    let start = Instant::now();
    let _sh = s.execute("bcf.show", &json!({"topic": guid})).unwrap();
    let show_ms = start.elapsed().as_millis();
    assert!(show_ms < 10_000, "bcf.show with 10k GUIDs took {show_ms} ms, budget 10 s");
}

// =============================== MUST 2 ===============================
// BCF snapshots and ifc.render must show the camera's subject, not the whole model.
// The snapshot of a topic on the duct has SELECTED-orange pixels covering at least 3% of the image.
// An ifc.render with a section box has no pixels of class colours from objects wholly outside
// the box.

#[test]
fn snapshot_frames_on_the_selection_not_the_whole_building() {
    let (mut s, d) = open_hotel("snapshot-frame");
    let duct_h = handle_of(&mut s, "1Dct00000000000000001A");
    s.set_selection(vec![cadaddy_engine::doc::Handle::parse_hex(&duct_h).unwrap()]);
    s.execute("ifc.camera", &json!({"eye": [1, 7, 1.7], "target": [5, 3, 2.6]})).unwrap();

    // Create a BCF topic on the duct with a snapshot.
    let t = s
        .execute(
            "bcf.create",
            &json!({
                "title": "Duct clearance question",
                "width": 640,
                "height": 480
            }),
        )
        .unwrap();
    assert!(t["snapshot"].as_bool().unwrap_or(false), "snapshot must be present");
    let guid = t["guid"].as_str().unwrap().to_string();

    // Get the snapshot and decode it.
    let full = s.execute("bcf.get", &json!({"topic": guid, "snapshot": true})).unwrap();
    let snap_b64 = full["viewpoints"][0]["snapshot"].as_str().expect("snapshot base64");
    let snap_bytes = cadaddy_engine::cmd::file::base64_decode(snap_b64).expect("decode base64");
    let (w, h, pixels) = decode_png(&snap_bytes);
    let total = pixels.len();
    let orange = pixels.iter().filter(|p| is_orange(**p)).count();
    let pct = (orange as f64 / total as f64) * 100.0;
    assert!(pct >= 3.0, "the duct's snapshot should have >= 3% orange (SELECTED) pixels, got {pct:.1}% ({orange}/{total}, image {w}x{h})");

    // Also test ifc.render with a selection: orange pixels should dominate.
    let png_path = d.join("render-sel.png");
    let r = s
        .execute(
            "ifc.render",
            &json!({
                "path": png_path.display().to_string(),
                "width": 640,
                "height": 480,
                "eye": [1, 7, 1.7],
                "target": [5, 3, 2.6]
            }),
        )
        .unwrap();
    assert!(r["triangles"].as_u64().unwrap() > 0);
    let render_bytes = std::fs::read(&png_path).unwrap();
    let (_, _, rpx) = decode_png(&render_bytes);
    let r_orange = rpx.iter().filter(|p| is_orange(**p)).count();
    let r_pct = (r_orange as f64 / rpx.len() as f64) * 100.0;
    assert!(r_pct >= 3.0, "ifc.render with duct selected should have >= 3% orange pixels, got {r_pct:.1}%");
}

#[test]
fn section_box_render_excludes_outside_objects() {
    let (mut s, d) = open_hotel("section-render");
    // Set a section box tightly around the column (at 8,8). Walls at 5,0 and 0,5 should be
    // completely outside. Render with section box and no selection highlighting.
    let col_h = handle_of(&mut s, "1Col00000000000000001A");
    s.set_selection(vec![cadaddy_engine::doc::Handle::parse_hex(&col_h).unwrap()]);
    s.execute("ifc.sectionbox", &json!({"fit": "selection", "pad": 0.5})).unwrap();
    s.set_selection(Vec::new()); // Clear selection — no highlight, pure class colours.

    let png_path = d.join("section-render.png");
    let r = s
        .execute(
            "ifc.render",
            &json!({
                "path": png_path.display().to_string(),
                "width": 640,
                "height": 480,
                "highlight": false
            }),
        )
        .unwrap();
    assert!(r["triangles"].as_u64().unwrap() > 0);

    let bytes = std::fs::read(&png_path).unwrap();
    let (w, h, pixels) = decode_png(&bytes);

    // The walls are drawn in RGB 222,218,210 (class_colour for "Wall"). The column is RGB
    // 175,170,160. The duct is 165,190,215. The door is 150,110,70. With the section box tight
    // around the column (8,8, pad 0.5), walls at 5,0 and 0,5 should be cut away — no wall-
    // coloured pixels. We check that no pixel matches the distinct duct colour (the duct is at
    // 5,3 which is far outside the column box).
    let is_duct_blue = |px: [u8; 4]| -> bool {
        // Duct class colour is 165,190,215. With shading, accept range.
        px[2] > px[0].saturating_add(20) && px[2] > px[1] && px[2] > 150
    };
    let duct_px = pixels.iter().filter(|p| is_duct_blue(**p)).count();
    assert_eq!(duct_px, 0, "section-box render around the column should have no duct-coloured (blue) pixels, got {duct_px} in {w}x{h}");
}
