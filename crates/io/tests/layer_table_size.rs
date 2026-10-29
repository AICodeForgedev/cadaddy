//! A DXF layer table of 100,000 layers loads in linear time (a name scan per record was
//! quadratic: 17 s for `info`).

use std::fmt::Write;
use std::time::Instant;

fn dxf_with_layers(n: usize) -> Vec<u8> {
    let mut s = String::from("0\nSECTION\n2\nHEADER\n9\n$ACADVER\n1\nAC1015\n0\nENDSEC\n0\nSECTION\n2\nTABLES\n0\nTABLE\n2\nLAYER\n");
    for i in 0..n {
        let _ = write!(s, "0\nLAYER\n2\nL{i}\n70\n0\n62\n7\n6\nCONTINUOUS\n");
    }
    s.push_str("0\nENDTAB\n0\nENDSEC\n0\nEOF\n");
    s.into_bytes()
}

#[test]
fn hundred_thousand_layers_load_fast() {
    let bytes = dxf_with_layers(100_000);
    let t = Instant::now();
    let d = cadaddy_io::read_dxf(&bytes).expect("reads");
    // Seconds before the fix: 17-23; now under one. 8 s leaves room for a loaded runner.
    assert!(t.elapsed().as_secs() < 8, "load took {:?}", t.elapsed());
    assert!(d.layers.len() >= 100_000);
    assert_eq!(d.layer("L99999").map(|l| l.name.as_str()), Some("L99999"));
}

#[test]
fn a_repeated_layer_name_keeps_the_last_record_once() {
    let mut s = String::from("0\nSECTION\n2\nTABLES\n0\nTABLE\n2\nLAYER\n");
    s.push_str("0\nLAYER\n2\nWalls\n70\n0\n62\n1\n6\nCONTINUOUS\n");
    s.push_str("0\nLAYER\n2\nWALLS\n70\n0\n62\n3\n6\nCONTINUOUS\n");
    s.push_str("0\nENDTAB\n0\nENDSEC\n0\nEOF\n");
    let d = cadaddy_io::read_dxf(s.as_bytes()).expect("reads");
    assert_eq!(d.layers.iter().filter(|l| l.name.eq_ignore_ascii_case("walls")).count(), 1);
}
