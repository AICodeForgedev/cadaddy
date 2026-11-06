//! Layer filters across a save: the LAYER_FILTER objects another program keeps under the layer
//! table's extension dictionary (ACAD_LAYERFILTERS) stay in the written file, CADaddy's own filters
//! come back equal, and filters that were deleted stay deleted.

use cadaddy_doc::{Drawing, Layer, LayerFilter, LayerFilterKind};

fn group(name: &str, layers: &[&str]) -> LayerFilter {
    LayerFilter { name: name.into(), parent: None, kind: LayerFilterKind::Group { layers: layers.iter().map(|l| (*l).to_string()).collect() } }
}

/// Group-code/value lines in the writer's own layout (right-aligned codes, CRLF).
fn tags(lines: &str) -> String {
    lines.lines().enumerate().map(|(i, l)| if i % 2 == 0 { format!("{l:>3}\r\n") } else { format!("{l}\r\n") }).collect()
}

/// A drawing saved by CADaddy, then given what another program writes: the LAYER table's extension
/// dictionary -> ACAD_LAYERFILTERS dictionary -> one LAYER_FILTER listing layers by group 8.
fn foreign_file() -> String {
    let mut d = Drawing::new_metric();
    for n in ["WALLS", "DOORS"] {
        d.layers.push(Layer::new(n));
    }
    let text = cadaddy_io::write_dxf(&d);
    let table = text.find("  2\r\nLAYER\r\n").unwrap();
    let hdr = table + text[table..].find("100\r\nAcDbSymbolTable").unwrap();
    let xdict = tags("102\n{ACAD_XDICTIONARY\n360\nFFE0\n102\n}");
    let text = format!("{}{}{}", &text[..hdr], xdict, &text[hdr..]);
    // The dictionary is owned by the LAYER table, as AutoCAD writes it.
    let owner = text[table..].split("\r\n").nth(3).unwrap().to_string();
    let objs = tags(&format!(
        "0\nDICTIONARY\n5\nFFE0\n330\n{owner}\n100\nAcDbDictionary\n3\nACAD_LAYERFILTERS\n350\nFFF0\n\
         0\nDICTIONARY\n5\nFFF0\n330\nFFE0\n100\nAcDbDictionary\n3\nWalls and doors\n350\nFFF1\n\
         0\nLAYER_FILTER\n5\nFFF1\n330\nFFF0\n100\nAcDbFilter\n100\nAcDbLayerFilter\n8\nWALLS\n8\nDOORS\n",
    ));
    let at = text.rfind("  0\r\nENDSEC").unwrap();
    format!("{}{}{}", &text[..at], objs, &text[at..])
}

#[test]
fn autocad_layer_filters_survive_a_save_with_ours_added() {
    let mut d = cadaddy_io::read_dxf(foreign_file().as_bytes()).unwrap();
    assert_eq!(d.layer_filters, [group("Walls and doors", &["WALLS", "DOORS"])]);
    d.layer_filters.push(group("Mine", &["DOORS"]));
    let written = cadaddy_io::write_dxf(&d);
    // The other program's objects are still in the file, with their names.
    assert!(written.contains("LAYER_FILTER"), "LAYER_FILTER object kept");
    assert!(written.contains("ACAD_LAYERFILTERS") && written.contains("Walls and doors"), "dictionary kept");
    let at = written.find("LAYER_FILTER").unwrap();
    let obj = &written[at..];
    let names: Vec<&str> = obj.split("\r\n").skip(1).take(16).collect();
    assert!(names.windows(2).any(|w| w[0].trim() == "8" && w[1] == "WALLS"), "{names:?}");
    assert!(names.windows(2).any(|w| w[0].trim() == "8" && w[1] == "DOORS"), "{names:?}");
    // And our filters come back equal.
    let back = cadaddy_io::read_dxf(written.as_bytes()).unwrap();
    assert_eq!(back.layer_filters, d.layer_filters);
}

#[test]
fn deleted_filters_stay_deleted_after_a_save() {
    let mut d = cadaddy_io::read_dxf(foreign_file().as_bytes()).unwrap();
    assert_eq!(d.layer_filters.len(), 1);
    // Deleting every filter must not let the other program's group filter come back on reopen.
    d.layer_filters.clear();
    let back = cadaddy_io::read_dxf(cadaddy_io::write_dxf(&d).as_bytes()).unwrap();
    assert_eq!(back.layer_filters, [], "0 filters after delete + save + reopen");
    // Same with a drawing that never had foreign filters: an emptied list stays empty.
    let mut ours = Drawing::new_metric();
    ours.layers.push(Layer::new("WALLS"));
    ours.layer_filters.push(group("Mine", &["WALLS"]));
    let once = cadaddy_io::read_dxf(cadaddy_io::write_dxf(&ours).as_bytes()).unwrap();
    assert_eq!(once.layer_filters.len(), 1);
    let mut cleared = once;
    cleared.layer_filters.clear();
    let back = cadaddy_io::read_dxf(cadaddy_io::write_dxf(&cleared).as_bytes()).unwrap();
    assert!(back.layer_filters.is_empty());
}
