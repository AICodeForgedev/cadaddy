//! Table formulas travel as AutoCAD FIELD objects (DXF Reference: ACAD_TABLE cell group 344, the
//! FIELD object's evaluator, field code and cached value), so other programs see the formula;
//! a file whose only formula record is the FIELD reads its formulas back. Wrapped cell texts are
//! written to the table's block with their wrapping width.

use cadaddy_doc::*;
use cadaddy_geom::Vec3;

fn drawing() -> (Drawing, Handle) {
    let mut d = Drawing::new_imperial();
    let cell = |t: &str| TableCell { text: t.into(), ..Default::default() };
    let mut cells = vec![vec![cell("a"), cell("2")], vec![cell("b"), cell("3")], vec![cell("Total"), cell("5")]];
    cells[2][1].formula = Some("=Sum(B1:B2)".into());
    cells[0][0].text = "a long description that wraps inside its narrow column".into();
    let t = Table {
        insert: Vec3::new(0.0, 0.0, 0.0),
        col_widths: vec![1.5, 2.0],
        row_heights: vec![0.4, 0.4, 0.4],
        cells,
        style: "Standard".into(),
        text_height: 0.18,
        ..Table::default()
    };
    let h = d.add(&Space::Model, Common::default(), EntityKind::Table(Box::new(t))).unwrap();
    (d, h)
}

fn groups(text: &str) -> Vec<(String, String)> {
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    lines.chunks(2).filter_map(|c| Some((c.first()?.to_string(), c.get(1)?.to_string()))).collect()
}

fn text_of(gs: &[(String, String)]) -> String {
    gs.iter().map(|(c, v)| format!("{c}\n{v}\n")).collect()
}

fn table(d: &Drawing, h: Handle) -> Table {
    match d.entity(h).map(|e| e.kind.clone()) {
        Some(EntityKind::Table(t)) => *t,
        k => panic!("not a table: {k:?}"),
    }
}

#[test]
fn formulas_are_written_as_field_objects() {
    let (d, h) = drawing();
    let text = cadaddy_io::write_dxf(&d);
    let gs = groups(&text);
    // The formula cell points at a FIELD; the FIELD holds an AcExpr child with the formula and
    // its cached value.
    let field = gs.iter().find(|(c, _)| c == "344").map(|(_, v)| v.clone()).expect("a 344 group");
    let at = gs.iter().position(|g| *g == ("5".to_string(), field.clone())).expect("the FIELD object");
    assert_eq!(gs.get(at - 1).map(|g| g.1.as_str()), Some("FIELD"));
    assert!(gs.contains(&("1".into(), "AcExpr".into())));
    assert!(gs.contains(&("2".into(), "\\AcExpr (Sum(B1:B2))".into())));
    assert!(gs.contains(&("7".into(), "ACFD_FIELD_VALUE".into())));
    assert!(gs.windows(2).any(|w| w[0] == ("90".into(), "2".into()) && w[1].0 == "140" && w[1].1.parse::<f64>().ok() == Some(5.0)));
    // Read back (CADaddy's own data and the field agree).
    let back = cadaddy_io::read_dxf(text.as_bytes()).unwrap();
    assert_eq!(table(&back, h).cells[2][1].formula.as_deref(), Some("=Sum(B1:B2)"));
}

#[test]
fn another_programs_table_gets_its_formula_from_the_field() {
    let (d, h) = drawing();
    let mut gs = groups(&cadaddy_io::write_dxf(&d));
    // Drop CADaddy's own xdata from the table (as another writer would have none).
    let start = gs.iter().position(|g| *g == ("5".to_string(), h.hex())).unwrap();
    let end = start + gs[start..].iter().skip(1).position(|(c, _)| c == "0").unwrap() + 1;
    let x = gs[start..end].iter().position(|g| *g == ("1001".to_string(), "CADADDY".to_string()));
    let x = x.expect("CADaddy's table xdata");
    gs.drain(start + x..end);
    let back = cadaddy_io::read_dxf(text_of(&gs).as_bytes()).unwrap();
    let t = table(&back, h);
    assert_eq!(t.cells[2][1].formula.as_deref(), Some("=Sum(B1:B2)"));
    assert_eq!(t.cells[2][1].text, "5");
    assert_eq!(t.cells[0][1].formula, None);
}

#[test]
fn hostile_fields_do_not_panic() {
    let (d, h) = drawing();
    let gs = groups(&cadaddy_io::write_dxf(&d));
    for (code, bad) in [("344", "ZZZ"), ("344", "0"), ("2", ""), ("2", "\\AcExpr ("), ("1", "Weird"), ("360", "FFFFFFFF")] {
        let mut g = gs.clone();
        for x in g.iter_mut().filter(|x| x.0 == code) {
            x.1 = bad.to_string();
        }
        let back = cadaddy_io::read_dxf(text_of(&g).as_bytes()).unwrap();
        let _ = back.entity(h);
    }
}

#[test]
fn wrapped_cell_text_keeps_its_width_in_the_block() {
    let (d, _) = drawing();
    let gs = groups(&cadaddy_io::write_dxf(&d));
    // An MTEXT in the table block with the long text and a width of the column less margins.
    let at = gs.iter().position(|(c, v)| c == "1" && v.starts_with("a long description")).expect("the cell text");
    let mtext = gs[..at].iter().rposition(|(c, v)| c == "0" && v == "MTEXT").unwrap();
    let width = gs[mtext..at].iter().find(|(c, _)| c == "41").map(|(_, v)| v.parse::<f64>().unwrap()).expect("a width");
    assert!(width > 0.0 && width < 1.5, "{width}");
}
