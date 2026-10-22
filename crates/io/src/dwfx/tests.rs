use super::*;
use cadaddy_doc::{Common, EntityKind, Line, Space};
use cadaddy_geom::Vec3;

// Content types (XPS 1.0 §9.1.2 and OPC).
const CT_FPAGE: &str = "application/vnd.ms-package.xps-fixedpage+xml";
const CT_FDSEQ: &str = "application/vnd.ms-package.xps-fixeddocumentsequence+xml";
const CT_FDOC: &str = "application/vnd.ms-package.xps-fixeddocument+xml";
const CT_RELS: &str = "application/vnd.openxmlformats-package.relationships+xml";
const CT_STRUCT: &str = "application/vnd.ms-package.xps-documentstructure+xml";
const CT_XML: &str = "application/xml";

fn line(a: (f64, f64), b: (f64, f64)) -> EntityKind {
    EntityKind::Line(Line { a: Vec3::new(a.0, a.1, 0.0), b: Vec3::new(b.0, b.1, 0.0) })
}

fn on(layer: &str) -> Common {
    Common { layer: layer.into(), ..Common::default() }
}

/// A one-page DWFx package around a hand-written FixedPage (no sequence: pages by number).
fn package(page: &str) -> Vec<u8> {
    zip(&[("[Content_Types].xml", CONTENT_TYPES.as_bytes(), false), ("Documents/1/Pages/1.fpage", page.as_bytes(), true)]).unwrap()
}

/// Each polyline of a page: its layer, points rounded to 1e-6 inch, closed.
fn shapes(d: &Drawing) -> Vec<(String, Vec<(f64, f64)>, bool)> {
    d.model
        .iter()
        .filter_map(|e| match &e.kind {
            EntityKind::LwPolyline(p) => Some((
                e.common.layer.to_string(),
                p.vertices.iter().map(|v| ((v.p.x * 1e6).round() / 1e6, (v.p.y * 1e6).round() / 1e6)).collect(),
                p.closed,
            )),
            _ => None,
        })
        .collect()
}

#[test]
fn minified_page_with_relative_paths_and_separate_subpaths() {
    // 96 × 192 units = 1 × 2 inches. Two subpaths in one Data; the first closed by `z`.
    let page = "<FixedPage xmlns='http://schemas.microsoft.com/xps/2005/06' Height='192' Width='96'><Path Stroke='#FF0000' Data='F1m0,0l48,0 0,48zM96,192h-96v-96'/></FixedPage>";
    let p = read_page(&package(page), 1).unwrap();
    assert_eq!(p.size, Vec2::new(1.0, 2.0));
    assert_eq!(p.pages, 1);
    assert_eq!(
        shapes(&p.drawing),
        [("0".to_string(), vec![(0.0, 2.0), (0.5, 2.0), (0.5, 1.5)], true), ("0".to_string(), vec![(1.0, 0.0), (0.0, 0.0), (0.0, 1.0)], false),]
    );
    let first = p.drawing.model.iter().next().unwrap();
    assert_eq!(first.common.color, cadaddy_color::Color::True(Rgb(255, 0, 0)));
}

#[test]
fn attributes_on_other_lines_arcs_and_nested_canvas_transforms() {
    let page = "<?xml version=\"1.0\"?>\n<FixedPage\n  Width=\"960\"\n  Height=\"960\"\n  xmlns=\"http://schemas.microsoft.com/xps/2005/06\">\n\
        <Canvas Name=\"A_x002D_WALL\" RenderTransform=\"1,0,0,1,96,0\">\n\
          <Canvas RenderTransform=\"2,0,0,2,0,0\">\n\
            <Path\n Data=\"M 0,480 A 48,48 0 0 1 96,480\"\n Stroke=\"#80000000\"/>\n\
          </Canvas>\n\
          <Path Data=\"M0,960 L96,960\">\n<Path.RenderTransform><MatrixTransform Matrix=\"1,0,0,1,0,-96\"/></Path.RenderTransform></Path>\n\
        </Canvas>\n\
        <Canvas Name=\"E-POWER\"><Path><Path.Data><PathGeometry><PathFigure StartPoint=\"0,0\" IsClosed=\"true\"><PolyLineSegment Points=\"96,0 96,96\"/></PathFigure></PathGeometry></Path.Data></Path></Canvas>\n\
        </FixedPage>";
    let p = read_page(&package(page), 1).unwrap();
    let s = shapes(&p.drawing);
    assert_eq!(s.len(), 3, "{s:?}");
    // The arc: page (0,480)-(96,480) scaled by 2 then moved by 96 → (96,960)-(288,960), i.e.
    // sheet (1,0)-(3,0) inches; a half circle of radius 1 inch about (2,0), on layer A-WALL.
    let (layer, arc, closed) = &s[0];
    assert_eq!((layer.as_str(), *closed), ("A-WALL", false));
    assert_eq!((arc[0], *arc.last().unwrap()), ((1.0, 0.0), (3.0, 0.0)));
    assert!(arc.iter().all(|q| ((q.0 - 2.0).hypot(q.1) - 1.0).abs() < 1e-6), "{arc:?}");
    // Sweep 1 on a y-down page bulges up the page: y > 0 in sheet inches.
    assert!(arc.iter().any(|q| q.1 > 0.99));
    // The Path's own transform after the canvas's: (0,960)-(96,960) up 96 then right 96.
    assert_eq!(s[1], ("A-WALL".to_string(), vec![(1.0, 1.0), (2.0, 1.0)], false));
    // A PathGeometry figure, closed, on the second canvas (a name not from CADaddy kept as is).
    assert_eq!(s[2], ("E-POWER".to_string(), vec![(0.0, 10.0), (1.0, 10.0), (1.0, 9.0)], true));
    let names: Vec<&str> = p.drawing.layers.iter().map(|l| l.name.as_str()).collect();
    assert!(names.contains(&"A-WALL") && names.contains(&"E-POWER"), "{names:?}");
}

#[test]
fn bad_pages_are_errors_not_panics() {
    for page in ["<FixedPage Width='96' Height='96'><Path Data='M0,0 L1'/></FixedPage>", "<FixedPage><Path Data='M0,0'>", "<Canvas/>", "not xml"] {
        assert!(read_page(&package(page), 1).is_err(), "{page}");
    }
    assert!(read_page(b"PK\x05\x06", 1).is_err());
    assert!(
        read_page(&package("<FixedPage Width='96' Height='96'/>"), 2).unwrap_err().to_string().contains("page 2 not found (the file has 1 page(s))")
    );
}

#[test]
fn canvas_names_are_xml_names_and_come_back() {
    for name in ["A-WALL", "0", "Defpoints", "my layer_1", "Ä|Ü", "1st", "\u{1F600}", ""] {
        let c = canvas_name(name);
        assert!(c.chars().next().is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '_'), "{c}");
        assert!(c.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_'), "{c}");
        let back = layer_from_canvas(&c);
        assert_eq!(back, if name.is_empty() { "0" } else { name });
    }
    assert_eq!(canvas_name("A-WALL"), "A_x002D_WALL");
    assert_eq!(canvas_name("a_1"), "a_x005F_1");
}

/// Two layers, a hyperlink on the WALLS line, a layout named "Sheet A1".
fn two_layer_drawing() -> Drawing {
    let mut d = Drawing::new_imperial();
    d.layers.push(Layer::new("WALLS"));
    d.layers.push(Layer::new("E-POWER"));
    let mut walls = on("WALLS");
    cadaddy_doc::hyperlink::write(
        &mut walls.xdata,
        Some(&cadaddy_doc::hyperlink::Hyperlink { url: "https://example.com/a?b=1&c=2".into(), ..Default::default() }),
    );
    d.add(&Space::Model, walls, line((0.0, 0.0), (10.0, 0.0))).unwrap();
    d.add(&Space::Model, on("E-POWER"), line((2.0, 3.0), (7.0, 6.0))).unwrap();
    d
}

#[test]
fn one_canvas_per_layer_with_hyperlinks_and_back_as_layers() {
    let d = two_layer_drawing();
    let bytes = export(&d, &Space::Model).unwrap();
    let page = part_xml(&bytes, "Documents/1/Pages/1.fpage").unwrap();
    let canvases: Vec<&str> = page.named("Canvas").filter_map(|c| c.attr("Name")).collect();
    assert_eq!(canvases, ["WALLS", "E_x002D_POWER"]);
    let walls = page.named("Canvas").next().unwrap();
    let paths: Vec<&xml::Element> = walls.named("Path").collect();
    assert_eq!(paths.len(), 1);
    assert_eq!(paths[0].attr("FixedPage.NavigateUri"), Some("https://example.com/a?b=1&c=2"));
    let power = page.named("Canvas").nth(1).unwrap();
    assert!(power.named("Path").all(|p| p.attr("FixedPage.NavigateUri").is_none()));

    let back = read_page(&bytes, 1).unwrap();
    let mut layers: Vec<String> = shapes(&back.drawing).into_iter().map(|s| s.0).collect();
    layers.dedup();
    assert_eq!(layers, ["WALLS", "E-POWER"]);
    let linked: Vec<String> = back.drawing.model.iter().filter_map(|e| cadaddy_doc::hyperlink::read(&e.common.xdata).map(|h| h.url)).collect();
    assert_eq!(linked, ["https://example.com/a?b=1&c=2"]);
}

#[test]
fn model_space_is_at_one_to_one_and_records_its_placement() {
    let d = two_layer_drawing();
    let p = read_page(&export(&d, &Space::Model).unwrap(), 1).unwrap();
    // Imperial: 1 drawing unit per sheet inch; half an inch of margin.
    let (origin, scale) = p.placement.unwrap();
    assert_eq!((origin, scale), (Vec2::new(-0.5, -0.5), 1.0));
    assert_eq!(p.size, Vec2::new(11.0, 7.0));
    assert_eq!(p.name.as_deref(), Some("Model"));
    // Page points + origin = the drawing's own points.
    let s = shapes(&p.drawing);
    assert_eq!(s[0].1, [(0.5, 0.5), (10.5, 0.5)]);
    assert_eq!(s[1].1, [(2.5, 3.5), (7.5, 6.5)]);
    // A stated scale: 2 drawing units per inch halves the page.
    let bytes = export_sheets_with(&d, &[Space::Model], &ExportOptions { scale: Some(2.0), fit: false }).unwrap();
    let p = read_page(&bytes, 1).unwrap();
    assert_eq!(p.placement, Some((Vec2::new(-1.0, -1.0), 2.0)));
    assert_eq!(shapes(&p.drawing)[0].1, [(0.5, 0.5), (5.5, 0.5)]);
    // Fitted only when asked: on 24 × 18 inches, the scale recorded.
    let p = read_page(&export_sheets_with(&d, &[Space::Model], &ExportOptions { scale: None, fit: true }).unwrap(), 1).unwrap();
    let (_, s) = p.placement.unwrap();
    assert!((s - 10.0 / 23.0).abs() < 1e-12, "{s}");
}

/// The relationships of a `.rels` part: (Id, Type, Target).
fn rel_list(bytes: &[u8], part: &str) -> Vec<(String, String, String)> {
    part_xml(bytes, part)
        .unwrap()
        .named("Relationship")
        .map(|r| (r.attr("Id").unwrap().to_string(), r.attr("Type").unwrap().to_string(), r.attr("Target").unwrap().to_string()))
        .collect()
}

#[test]
fn dwfx_is_a_valid_xps_package() {
    let mut d = two_layer_drawing();
    let layout = d.layouts[0].name.clone();
    d.add(&Space::Paper(layout.clone()), Common::default(), line((1.0, 1.0), (2.0, 2.0))).unwrap();
    let bytes = export_sheets(&d, &[Space::Model, Space::Paper(layout)]).unwrap();
    let parts = zip_list(&bytes).unwrap();

    // [Content_Types].xml gives every other part its content type (by extension or override).
    let ct = part_xml(&bytes, "[Content_Types].xml").unwrap();
    assert_eq!(ct.name, "Types");
    let defaults: HashMap<String, String> =
        ct.named("Default").map(|e| (e.attr("Extension").unwrap().to_ascii_lowercase(), e.attr("ContentType").unwrap().to_string())).collect();
    let overrides: HashMap<String, String> =
        ct.named("Override").map(|e| (e.attr("PartName").unwrap().to_string(), e.attr("ContentType").unwrap().to_string())).collect();
    let content_type = |part: &str| -> String {
        overrides.get(&format!("/{part}")).cloned().unwrap_or_else(|| {
            let ext = part.rsplit('.').next().unwrap().to_ascii_lowercase();
            defaults.get(&ext).cloned().unwrap_or_else(|| panic!("no content type for {part}"))
        })
    };
    let want = [
        ("_rels/.rels", CT_RELS),
        ("FixedDocumentSequence.fdseq", CT_FDSEQ),
        ("Documents/1/FixedDocument.fdoc", CT_FDOC),
        ("Documents/1/_rels/FixedDocument.fdoc.rels", CT_RELS),
        ("Documents/1/Pages/1.fpage", CT_FPAGE),
        ("Documents/1/Pages/2.fpage", CT_FPAGE),
        ("Documents/1/Structure/DocStructure.struct", CT_STRUCT),
        ("Documents/1/CADaddySheets.xml", CT_XML),
    ];
    let mut listed: Vec<&str> = parts.iter().map(String::as_str).filter(|p| *p != "[Content_Types].xml").collect();
    listed.sort();
    let mut expect: Vec<&str> = want.iter().map(|(p, _)| *p).collect();
    expect.sort();
    assert_eq!(listed, expect, "exactly the XPS parts");
    for (part, ty) in want {
        assert_eq!(content_type(part), ty, "{part}");
    }

    // Every relationship's target is a part of the package with the content type its type needs.
    let root = rel_list(&bytes, "_rels/.rels");
    assert_eq!(root, [("R0".to_string(), FDSEQ_REL.to_string(), "/FixedDocumentSequence.fdseq".to_string())]);
    let doc = rel_list(&bytes, "Documents/1/_rels/FixedDocument.fdoc.rels");
    assert_eq!(doc.len(), 2);
    for (_, ty, target) in root.iter().chain(&doc) {
        let part = target.trim_start_matches('/');
        assert!(parts.iter().any(|p| p == part), "{target} is in the package");
        let need = match ty.as_str() {
            FDSEQ_REL => CT_FDSEQ,
            STRUCT_REL => CT_STRUCT,
            SHEETS_REL => CT_XML,
            other => panic!("unexpected relationship {other}"),
        };
        assert_eq!(content_type(part), need, "{target}");
    }
    // The sequence names the document, the document its pages, in order, and each page is an
    // XPS FixedPage of the size the document gives.
    let seq = part_xml(&bytes, "FixedDocumentSequence.fdseq").unwrap();
    assert_eq!((seq.name.as_str(), seq.attr("xmlns")), ("FixedDocumentSequence", Some(XPS_NS)));
    assert_eq!(seq.named("DocumentReference").filter_map(|r| r.attr("Source")).collect::<Vec<_>>(), ["/Documents/1/FixedDocument.fdoc"]);
    let fdoc = part_xml(&bytes, "Documents/1/FixedDocument.fdoc").unwrap();
    let pcs: Vec<&xml::Element> = fdoc.named("PageContent").collect();
    assert_eq!(pcs.iter().filter_map(|p| p.attr("Source")).collect::<Vec<_>>(), ["/Documents/1/Pages/1.fpage", "/Documents/1/Pages/2.fpage"]);
    for (i, pc) in pcs.iter().enumerate() {
        let page = part_xml(&bytes, pc.attr("Source").unwrap()).unwrap();
        assert_eq!((page.name.as_str(), page.attr("xmlns")), ("FixedPage", Some(XPS_NS)));
        assert_eq!((page.attr("Width"), page.attr("Height")), (pc.attr("Width"), pc.attr("Height")));
        assert_eq!(page.attr("Name"), Some(format!("Sheet_{}", i + 1).as_str()));
    }
}

#[test]
fn sheets_are_named_in_the_document_outline() {
    let mut d = Drawing::new_metric();
    let first = d.layouts[0].name.clone();
    d.layouts.push(cadaddy_doc::Layout::new("Plan & Section", 2));
    let bytes = export_sheets(&d, &[Space::Paper(first.clone()), Space::Paper("Plan & Section".into())]).unwrap();
    let st = part_xml(&bytes, "Documents/1/Structure/DocStructure.struct").unwrap();
    let mut entries = Vec::new();
    st.descendants("OutlineEntry", &mut entries);
    let got: Vec<(&str, &str)> = entries.iter().map(|e| (e.attr("Description").unwrap(), e.attr("OutlineTarget").unwrap())).collect();
    assert_eq!(got, [(first.as_str(), "/Documents/1/FixedDocument.fdoc#Sheet_1"), ("Plan & Section", "/Documents/1/FixedDocument.fdoc#Sheet_2")]);
    assert_eq!(read_page(&bytes, 2).unwrap().name.as_deref(), Some("Plan & Section"));
    assert_eq!(read_page(&bytes, 2).unwrap().pages, 2);
}

#[test]
fn a_part_expanding_past_the_limit_is_refused() {
    // 65 MiB of zeros deflate to about 64 KiB; the part says it is 1 byte.
    let big = vec![0u8; MAX_PART + (1 << 20)];
    let packed = miniz_oxide::deflate::compress_to_vec(&big, 6);
    assert!(packed.len() < 1 << 20);
    let mut bytes = zip(&[("Documents/1/Pages/1.fpage", b"x".as_slice(), false)]).unwrap();
    // Swap the stored 1-byte part for the deflate stream, keeping the declared sizes small.
    let name = b"Documents/1/Pages/1.fpage";
    let data_at = 30 + name.len();
    bytes.splice(data_at..data_at + 1, packed.iter().copied());
    let shift = packed.len() - 1;
    let csize = u32::try_from(packed.len()).unwrap().to_le_bytes();
    bytes[8..10].copy_from_slice(&8u16.to_le_bytes());
    bytes[18..22].copy_from_slice(&csize);
    let cd = data_at + packed.len();
    bytes[cd + 10..cd + 12].copy_from_slice(&8u16.to_le_bytes());
    bytes[cd + 20..cd + 24].copy_from_slice(&csize);
    let eocd = bytes.len() - 22;
    let cd_at = u32::from_le_bytes(bytes[eocd + 16..eocd + 20].try_into().unwrap()) as usize + shift;
    bytes[eocd + 16..eocd + 20].copy_from_slice(&u32::try_from(cd_at).unwrap().to_le_bytes());
    let e = zip_read(&bytes, "Documents/1/Pages/1.fpage").unwrap_err().to_string();
    assert!(e.contains("expands past 64 MiB"), "{e}");
    assert!(read_page(&bytes, 1).is_err());
}

#[test]
fn dwf6_has_its_header_and_decodes_to_the_source_coordinates() {
    let d = two_layer_drawing();
    let bytes = export_dwf6(&d, &Space::Model).unwrap();
    assert!(bytes.starts_with(b"(DWF V06.00)PK\x03\x04"));
    let zipped = &bytes[12..];
    assert_eq!(zip_list(zipped).unwrap(), ["manifest.xml", "ePlot_1/graphics.w2d"]);
    let w2d = zip_read(zipped, "ePlot_1/graphics.w2d").unwrap();
    let text = String::from_utf8(w2d).unwrap();
    // Imperial model space at 1:1 with half an inch of margin: (0,0) is 5000,5000.
    assert!(text.contains("(Layer 1 'WALLS')\n(Color 0,0,0,255)\nP 2 5000,5000 105000,5000\n"), "{text}");
    assert!(text.contains("(Layer 2 'E-POWER')\nP 2 25000,35000 75000,65000\n"), "{text}");
    let p = read_page(&bytes, 1).unwrap();
    assert_eq!((p.pages, p.name.as_deref(), p.size), (1, Some("Model"), Vec2::new(11.0, 7.0)));
    let (origin, scale) = p.placement.unwrap();
    let back: Vec<(String, Vec<(f64, f64)>)> = shapes(&p.drawing)
        .into_iter()
        .map(|(l, pts, _)| (l, pts.iter().map(|q| (q.0 * scale + origin.x, q.1 * scale + origin.y)).collect()))
        .collect();
    assert_eq!(back, [("WALLS".to_string(), vec![(0.0, 0.0), (10.0, 0.0)]), ("E-POWER".to_string(), vec![(2.0, 3.0), (7.0, 6.0)])]);
}

#[test]
fn empty_drawing_exports() {
    let d = Drawing::new_metric();
    let bytes = export(&d, &Space::Model).unwrap();
    assert!(bytes.starts_with(b"PK\x03\x04"));
    assert_eq!(read_page(&bytes, 1).unwrap().drawing.model.iter().count(), 0);
    assert!(export_sheets(&d, &[]).is_err());
}
