//! `info` prints the drawing's units by their INSUNITS name, as `info --help` promises (it always
//! printed "?" before; battle test 7, B10-013).

use std::process::{Command, Output};

fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cadaddy-cli")).args(args).output().unwrap()
}

fn units(dxf: &str, name: &str) -> serde_json::Value {
    let dir = std::env::temp_dir().join(format!("cc-cli-units-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, dxf).unwrap();
    let out = cli(&["info", path.to_str().unwrap()]);
    assert!(out.status.success(), "{:?}: {}", out.status, String::from_utf8_lossy(&out.stderr));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    v["units"].clone()
}

const BODY: &str = "0\nSECTION\n2\nENTITIES\n0\nLINE\n8\n0\n10\n0\n20\n0\n11\n5\n21\n5\n0\nENDSEC\n0\nEOF\n";

#[test]
fn info_prints_the_drawing_units() {
    let mm = format!("0\nSECTION\n2\nHEADER\n9\n$INSUNITS\n70\n4\n0\nENDSEC\n{BODY}");
    assert_eq!(units(&mm, "mm.dxf"), serde_json::json!("Millimeters"));
    let ft = format!("0\nSECTION\n2\nHEADER\n9\n$INSUNITS\n70\n2\n0\nENDSEC\n{BODY}");
    assert_eq!(units(&ft, "ft.dxf"), serde_json::json!("Feet"));
    assert_eq!(units(BODY, "none.dxf"), serde_json::json!("Unitless"));
}
