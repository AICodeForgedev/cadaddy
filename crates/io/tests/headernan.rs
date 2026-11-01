//! Battle test 6, B9-016: a non-finite header number is dropped on reading (named once) and never
//! written; an old .cdy whose header holds a bad variable opens with its other settings, and
//! `verify` says the header is the problem.

use cadaddy_doc::{Drawing, HVal};

fn dxf_with_header(vars: &str) -> Vec<u8> {
    format!("0\nSECTION\n2\nHEADER\n{vars}0\nENDSEC\n0\nSECTION\n2\nENTITIES\n0\nENDSEC\n0\nEOF\n").into_bytes()
}

#[test]
fn dxf_header_nonfinite_numbers_keep_the_default_and_warn_once() {
    let d = cadaddy_io::read_dxf(&dxf_with_header(
        "9\n$INSUNITS\n70\n4\n9\n$THICKNESS\n40\nnan\n9\n$ELEVATION\n40\ninf\n9\n$LTSCALE\n40\ninf\n9\n$DIMSCALE\n40\nnan\n9\n$TEXTSIZE\n40\n-inf\n",
    ))
    .unwrap();
    assert_eq!(d.header.i64("INSUNITS", 0), 4);
    for v in ["THICKNESS", "ELEVATION", "LTSCALE", "DIMSCALE", "TEXTSIZE"] {
        assert!(d.header.get(v).is_none_or(|h| h.as_f64().is_some_and(f64::is_finite)), "{v} kept a non-finite value");
    }
    let said: Vec<&String> = d.read_notes.iter().filter(|n| n.contains("header variable")).collect();
    assert_eq!(said.len(), 1, "{:?}", d.read_notes);
    for v in ["$THICKNESS", "$ELEVATION", "$LTSCALE", "$DIMSCALE", "$TEXTSIZE"] {
        assert!(said[0].contains(v), "{v} not named: {}", said[0]);
    }
    // Converting to .cdy writes a header that reads back whole.
    let cdy = cadaddy_io::cdy::write_cdy(&d).unwrap();
    let (back, rep) = cadaddy_io::cdy::read_cdy_report(&cdy).unwrap();
    assert!(rep.notes.is_empty(), "{:?}", rep.notes);
    assert_eq!(back.header.i64("INSUNITS", 0), 4);
}

#[test]
fn dxf_text_with_a_nonfinite_height_is_warned_not_hidden() {
    let dxf = "0\nSECTION\n2\nENTITIES\n0\nTEXT\n8\n0\n10\n0\n20\n0\n40\nnan\n1\nhello\n0\nENDSEC\n0\nEOF\n";
    let d = cadaddy_io::read_dxf(dxf.as_bytes()).unwrap();
    assert!(d.read_notes.iter().any(|n| n.contains("text height") && n.contains("NaN")), "{:?}", d.read_notes);
    let h = d.model.iter().find_map(|e| match &e.kind {
        cadaddy_doc::EntityKind::Text(t) => Some(t.height),
        _ => None,
    });
    assert_eq!(h, Some(0.2));
}

#[test]
fn the_cdy_writer_leaves_out_a_variable_its_reader_cant_read() {
    let mut d = Drawing::new_metric();
    d.header.set_i64("INSUNITS", 4);
    d.header.set_f64("DIMSCALE", 48.0);
    d.header.set("THICKNESS", HVal::Real(f64::NAN));
    d.header.set("PDSIZE", HVal::Real(f64::NEG_INFINITY));
    let bytes = cadaddy_io::cdy::write_cdy(&d).unwrap();
    let (back, rep) = cadaddy_io::cdy::read_cdy_report(&bytes).unwrap();
    assert!(rep.notes.is_empty(), "{:?}", rep.notes);
    assert_eq!((back.header.i64("INSUNITS", 0), back.header.f64("DIMSCALE", 1.0)), (4, 48.0));
    assert!(back.header.get("THICKNESS").is_none() && back.header.get("PDSIZE").is_none());
    assert_eq!(cadaddy_io::cdy::verify(&bytes).unwrap()["ok"], true);
}
