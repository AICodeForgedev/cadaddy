//! The DWG engine switch (Q203): which engine `CADADDY_DWG` / `CADADDY_DWG_READ` /
//! `CADADDY_DWG_WRITE` choose, and the fallback to the bridge when CADaddy's own reader
//! rejects a file.

use cadaddy_io::{DwgEngine, dwg_engine_from, read_dwg};
use std::collections::HashMap;

fn engine(read: bool, vars: &[(&str, &str)]) -> DwgEngine {
    let map: HashMap<String, String> = vars.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    dwg_engine_from(read, |k| map.get(k).cloned())
}

#[test]
fn defaults_apply_with_no_variables() {
    assert_eq!(engine(true, &[]), cadaddy_io::DEFAULT_DWG_READ);
    assert_eq!(engine(false, &[]), cadaddy_io::DEFAULT_DWG_WRITE);
}

#[test]
fn the_shared_variable_picks_both_directions() {
    assert_eq!(engine(true, &[("CADADDY_DWG", "native")]), DwgEngine::Native);
    assert_eq!(engine(false, &[("CADADDY_DWG", "native")]), DwgEngine::Native);
    assert_eq!(engine(true, &[("CADADDY_DWG", "acadrust")]), DwgEngine::Bridge);
    assert_eq!(engine(false, &[("CADADDY_DWG", "bridge")]), DwgEngine::Bridge);
}

#[test]
fn a_per_direction_variable_beats_the_shared_one() {
    let v = [("CADADDY_DWG", "native"), ("CADADDY_DWG_READ", "bridge"), ("CADADDY_DWG_WRITE", "acadrust")];
    assert_eq!(engine(true, &v), DwgEngine::Bridge);
    assert_eq!(engine(false, &v), DwgEngine::Bridge);
    let v = [("CADADDY_DWG", "acadrust"), ("CADADDY_DWG_READ", "native"), ("CADADDY_DWG_WRITE", "native")];
    assert_eq!(engine(true, &v), DwgEngine::Native);
    assert_eq!(engine(false, &v), DwgEngine::Native);
    // Each direction only reads its own variable.
    assert_eq!(engine(true, &[("CADADDY_DWG_WRITE", "native"), ("CADADDY_DWG", "bridge")]), DwgEngine::Bridge);
    assert_eq!(engine(false, &[("CADADDY_DWG_READ", "native"), ("CADADDY_DWG", "bridge")]), DwgEngine::Bridge);
    assert_eq!(engine(true, &[("CADADDY_DWG_READ", "native")]), DwgEngine::Native);
}

#[test]
fn case_and_whitespace_do_not_matter() {
    assert_eq!(engine(true, &[("CADADDY_DWG", "  NATIVE ")]), DwgEngine::Native);
    assert_eq!(engine(false, &[("CADADDY_DWG_WRITE", "\tNative\n")]), DwgEngine::Native);
    assert_eq!(engine(true, &[("CADADDY_DWG", " AcadRust ")]), DwgEngine::Bridge);
    assert_eq!(engine(true, &[("CADADDY_DWG_READ", " BRIDGE ")]), DwgEngine::Bridge);
}

#[test]
fn an_unknown_value_falls_through() {
    assert_eq!(engine(true, &[("CADADDY_DWG", "fast")]), cadaddy_io::DEFAULT_DWG_READ);
    assert_eq!(engine(false, &[("CADADDY_DWG", "")]), cadaddy_io::DEFAULT_DWG_WRITE);
    // An unknown per-direction value falls to the shared variable, then to the default.
    assert_eq!(engine(true, &[("CADADDY_DWG_READ", "bogus"), ("CADADDY_DWG", "native")]), DwgEngine::Native);
    assert_eq!(engine(true, &[("CADADDY_DWG_READ", "bogus"), ("CADADDY_DWG", "bogus")]), cadaddy_io::DEFAULT_DWG_READ);
}

/// A made-up DWG of `n` lines, cut short.
fn cut_dwg() -> Vec<u8> {
    let mut src = String::from("0\nSECTION\n2\nENTITIES\n");
    for i in 0..2000 {
        src += &format!("0\nLINE\n8\nWALLS\n10\n{i}\n20\n0\n30\n0\n11\n{i}\n21\n7\n31\n0\n");
    }
    src += "0\nENDSEC\n0\nEOF\n";
    let mut d = cadaddy_io::read(src.as_bytes(), "made-up.dxf").unwrap();
    d.header.set_str("ACADVER", "AC1032");
    let full = cadaddy_io::write(&d, "made-up.dwg").unwrap();
    full[..full.len() * 9 / 10].to_vec()
}

/// CADaddy's reader rejects the file; the bridge salvages it, and the drawing says which reader
/// was used.
#[test]
fn a_file_the_codec_rejects_is_read_by_the_bridge_with_a_note() {
    let bytes = cut_dwg();
    assert!(cadaddy_io::dwg_read::read(&bytes).is_err(), "the codec was expected to reject the cut file");
    let d = read_dwg(&bytes, DwgEngine::Native).expect("the bridge fallback reads it");
    assert!(d.entity_count() > 0);
    assert!(d.read_notes[0].starts_with("DWG: read through the DWG bridge"), "{:?}", d.read_notes);
}
