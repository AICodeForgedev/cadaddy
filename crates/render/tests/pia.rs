//! Binary plot style tables (.ctb / .stb): what we write reads back the same; damaged files are
//! errors, not crashes. Only files this code writes are used.

use cadaddy_color::Rgb;
use cadaddy_doc::{EndStyle, FillStyle, JoinStyle, PlotStyle, PlotStyleKind, PlotStyleTable};
use cadaddy_render::pia;

#[test]
fn built_in_tables_round_trip() {
    for name in cadaddy_doc::BUILTIN_PLOT_STYLE_TABLES {
        let t = PlotStyleTable::builtin(name).unwrap();
        let bytes = pia::write(&t);
        assert!(pia::is_pia(&bytes));
        let back = pia::read(&bytes, name).unwrap();
        assert_eq!(back, t, "{name}");
    }
}

#[test]
fn every_field_survives() {
    let style = PlotStyle {
        name: "Walls heavy".into(),
        description: "for walls\\nsecond line".into(),
        color: Some(Rgb(12, 200, 99)),
        grayscale: true,
        screening: 40,
        linetype: Some("Dashed".into()),
        lineweight: Some(70),
        end: EndStyle::Round,
        join: JoinStyle::Bevel,
        fill: FillStyle::Crosshatch,
    };
    let t = PlotStyleTable::named("mine.stb", "A test table", vec![style.clone()]);
    let back = pia::read(&pia::write(&t), "mine.stb").unwrap();
    assert_eq!(back.kind, PlotStyleKind::Named);
    assert_eq!(back.description, "A test table");
    let s = back.styles.iter().find(|s| s.name == "Walls heavy").unwrap();
    assert_eq!(s, &style);
    // A lineweight between the standard ones comes back as the nearest.
    let odd = PlotStyleTable::named("odd.stb", "", vec![PlotStyle { name: "Odd".into(), lineweight: Some(33), ..PlotStyle::default() }]);
    let s = pia::read(&pia::write(&odd), "odd.stb").unwrap().styles.into_iter().find(|s| s.name == "Odd").unwrap();
    assert_eq!(s.lineweight, Some(35));
    // A colour-dependent table says so in its header and its text.
    let ctb = pia::write(&PlotStyleTable::builtin("monochrome.ctb").unwrap());
    assert!(ctb.starts_with(b"PIAFILEVERSION_2.0,CTBVER1,compress\r\npmzlibcodec"));
    assert!(pia::body(&PlotStyleTable::builtin("monochrome.ctb").unwrap()).contains("aci_table_available=TRUE"));
}

#[test]
fn damaged_files_are_errors() {
    assert!(pia::read(b"", "x.ctb").is_err());
    assert!(pia::read(b"hello", "x.ctb").is_err());
    let mut bytes = pia::write(&PlotStyleTable::builtin("default.stb").unwrap());
    assert!(pia::read(bytes.get(..50).unwrap(), "x.stb").is_err(), "cut in the header");
    let n = bytes.len();
    for b in bytes.iter_mut().skip(70).take(n.saturating_sub(80)) {
        *b ^= 0x5a;
    }
    let _ = pia::read(&bytes, "x.stb");
    // Huge input is refused before inflating.
    let mut big = b"PIAFILEVERSION_2.0,CTBVER1,compress\r\npmzlibcodec".to_vec();
    big.resize(5 << 20, 0);
    assert!(pia::read(&big, "big.ctb").is_err());
}
