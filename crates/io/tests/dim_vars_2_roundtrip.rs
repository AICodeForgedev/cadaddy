//! DIMTFILL, DIMTFILLCLR, DIMTXTDIRECTION, DIMUPT, DIMALTTD, DIMALTTZ and the jog height and break
//! size settings survive DXF in every version: as DIMSTYLE groups where the version has them,
//! else in the style's `CADADDY` "DIMVARS" xdata. DIMTFILLCLR is always in the xdata (its code,
//! 70, is the table record's flags group), and the two settings without a DIMSTYLE group too.
//! Dimension overrides of the six variables travel in the `ACAD` DSTYLE xdata.

use cadaddy_color::Color;
use cadaddy_doc::{Common, DimKind, DimStyle, Dimension, Drawing, EntityKind, Space};
use cadaddy_geom::Vec3;
use cadaddy_io::{SaveOptions, Version, read, write_with};

fn styled() -> DimStyle {
    DimStyle {
        name: "Filled".into(),
        text_fill: 2,
        text_fill_color: Color::Index(1),
        text_right_to_left: true,
        user_text_places_line: true,
        alt_tol_decimals: 4,
        alt_tol_zero_suppression: 8,
        jog_height: 2.25,
        break_size: 0.3,
        ..DimStyle::default()
    }
}

fn drawing() -> Drawing {
    let mut d = Drawing::new_imperial();
    d.dim_styles.push(styled());
    let dim = Dimension {
        kind: DimKind::Linear { rotation: 0.0 },
        defpt: Vec3::new(0.0, 3.0, 0.0),
        text_mid: Vec3::ZERO,
        p13: Vec3::ZERO,
        p14: Vec3::new(10.0, 0.0, 0.0),
        p15: Vec3::ZERO,
        p16: Vec3::ZERO,
        text: String::new(),
        style: "Standard".into(),
        measurement: 0.0,
        text_rotation: 0.0,
        user_text_pos: false,
        block: None,
        overrides: serde_json::json!({ "textFill": 2, "textFillColor": 5, "altTolDecimals": 1, "userTextPlacesLine": true })
            .as_object()
            .cloned()
            .unwrap_or_default(),
        assoc: Vec::new(),
        marks: Default::default(),
    };
    d.add(&Space::Model, Common::default(), EntityKind::Dimension(Box::new(dim))).unwrap();
    d
}

fn dxf(d: &Drawing, v: Version) -> Vec<u8> {
    write_with(d, "t.dxf", &SaveOptions { version: Some(v), binary: false }).unwrap()
}

/// The groups of the "Filled" DIMSTYLE record, up to its xdata.
fn record_groups(text: &str) -> Vec<(i32, String)> {
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    let pairs: Vec<(i32, String)> = lines.chunks(2).filter_map(|c| Some((c.first()?.parse().ok()?, c.get(1)?.to_string()))).collect();
    let start = pairs.iter().position(|(c, v)| *c == 2 && v == "Filled").expect("the record");
    pairs[start..].iter().take_while(|(c, _)| *c != 0 && *c != 1001).cloned().collect()
}

#[test]
fn new_variables_roundtrip_in_every_dxf_version() {
    let d = drawing();
    let want = styled();
    for v in Version::ALL {
        let bytes = dxf(&d, v);
        let back = read(&bytes, "t.dxf").unwrap();
        assert_eq!(back.dim_style("Filled"), Some(&want), "{v:?}");
        // The record has one flags group (70); the fill colour isn't a second one.
        let text = String::from_utf8_lossy(&bytes);
        let groups = record_groups(&text);
        assert_eq!(groups.iter().filter(|(c, _)| *c == 70).count(), 1, "{v:?}: {groups:?}");
        // DIMTFILL is a group from 2007, DIMTXTDIRECTION from 2010; DIMUPT/DIMALTTD/DIMALTTZ always.
        let has = |code: i32| groups.iter().any(|(c, _)| *c == code);
        assert_eq!(has(69), v >= Version::R2007, "{v:?} DIMTFILL");
        assert_eq!(has(294), v >= Version::R2010, "{v:?} DIMTXTDIRECTION");
        assert!(has(288) && has(274) && has(286), "{v:?}");
        // CADaddy's own codes never appear as groups.
        assert!(!has(1388) && !has(1391), "{v:?}");
    }
}

#[test]
fn overrides_of_the_new_variables_roundtrip_in_dstyle() {
    let d = drawing();
    for v in [Version::R2000, Version::R2018] {
        let back = read(&dxf(&d, v), "t.dxf").unwrap();
        let dm =
            back.model.iter().find_map(|e| if let EntityKind::Dimension(dm) = &e.kind { Some(dm.clone()) } else { None }).expect("the dimension");
        let st = DimStyle::default().with_overrides(&dm.overrides);
        assert_eq!((st.text_fill, st.text_fill_color, st.alt_tol_decimals, st.user_text_places_line), (2, Color::Index(5), 1, true), "{v:?}");
    }
}

#[test]
fn a_flags_group_is_never_read_as_the_fill_colour() {
    // A record whose flags are 16 (xref-dependent) keeps DIMTFILLCLR at its default.
    let mut d = Drawing::new_imperial();
    d.dim_styles.push(DimStyle { name: "Flagged".into(), ..DimStyle::default() });
    let text = String::from_utf8_lossy(&dxf(&d, Version::R2018)).to_string();
    let at = text.find("\nFlagged").expect("record");
    let (head, tail) = text.split_at(at);
    let tail = tail.replacen("\n 70\n0\n", "\n 70\n16\n", 1).replacen("\n 70\r\n0\r\n", "\n 70\r\n16\r\n", 1);
    let back = read(format!("{head}{tail}").as_bytes(), "t.dxf").unwrap();
    assert_eq!(back.dim_style("Flagged").map(|s| s.text_fill_color), Some(Color::ByBlock));
}

#[test]
fn dwg_keeps_the_new_style_variables() {
    let d = drawing();
    let bytes = write_with(&d, "t.dwg", &SaveOptions::default()).unwrap();
    let back = read(&bytes, "t.dwg").unwrap();
    assert_eq!(back.dim_style("Filled"), Some(&styled()));
}
