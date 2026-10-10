//! Plot style tables in plots and exports (issue #410): the PDF content stream carries the
//! table's colours and pen widths, layout SVG exports follow "Display plot styles", and the
//! page setup's table and flag survive a DXF round trip.

use cadcraft_color::Color;
use cadcraft_doc::{Common, Drawing, EntityKind, Line, Space};
use cadcraft_geom::Vec3;
use cadcraft_io::{read_dxf, svg, write_dxf};
use serde_json::json;

fn layout_with_red_line() -> Drawing {
    let mut d = Drawing::new_metric();
    let line = EntityKind::Line(Line { a: Vec3::new(10.0, 10.0, 0.0), b: Vec3::new(100.0, 10.0, 0.0) });
    d.add(&Space::Paper("Layout1".into()), Common { color: Color::Index(1), ..Default::default() }, line).unwrap();
    d
}

/// The uncompressed PDF of Layout1 with extra plot options.
fn pdf_text(d: &Drawing, extra: serde_json::Value) -> String {
    let mut opts = json!({"compress": false});
    if let (Some(o), Some(e)) = (opts.as_object_mut(), extra.as_object()) {
        o.extend(e.clone());
    }
    let bytes = cadcraft_io::plot(d, &Space::Paper("Layout1".into()), &opts).unwrap();
    String::from_utf8_lossy(&bytes).into_owned()
}

#[test]
fn pdf_uses_the_page_setup_table() {
    let mut d = layout_with_red_line();
    assert!(pdf_text(&d, json!({})).contains("1 0 0 RG"), "no table: red");
    d.layouts[0].page.plot_style_table = "monochrome.ctb".into();
    let mono = pdf_text(&d, json!({}));
    assert!(mono.contains("0 0 0 RG") && !mono.contains("1 0 0 RG"), "{mono}");
    // The plot's own option replaces the page setup's; "None" plots object colours.
    assert!(pdf_text(&d, json!({"plotStyleTable": "None"})).contains("1 0 0 RG"));
    let gray = pdf_text(&d, json!({"plotStyleTable": "grayscale.ctb"}));
    assert!(!gray.contains("1 0 0 RG") && !gray.contains("0 0 0 RG"), "red prints mid grey: {gray}");
}

#[test]
fn layout_svg_follows_display_plot_styles() {
    let mut d = layout_with_red_line();
    d.layouts[0].page.plot_style_table = "monochrome.ctb".into();
    let space = Space::Paper("Layout1".into());
    assert!(svg::export(&d, &space).contains("#ff0000"));
    d.layouts[0].page.show_plot_styles = true;
    let s = svg::export(&d, &space);
    assert!(!s.contains("#ff0000") && s.contains("#000000"), "{s}");
}

#[test]
fn page_setup_table_and_display_flag_round_trip() {
    let mut d = layout_with_red_line();
    d.layouts[0].page.plot_style_table = "grayscale.ctb".into();
    d.layouts[0].page.show_plot_styles = true;
    let back = read_dxf(write_dxf(&d).as_bytes()).unwrap();
    let page = &back.layout("Layout1").unwrap().page;
    assert_eq!(page.plot_style_table, "grayscale.ctb");
    assert!(page.show_plot_styles);
    assert!(!read_dxf(write_dxf(&layout_with_red_line()).as_bytes()).unwrap().layout("Layout1").unwrap().page.show_plot_styles);
}
