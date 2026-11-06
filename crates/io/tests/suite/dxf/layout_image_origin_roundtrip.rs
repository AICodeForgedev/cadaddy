//! A layout's page keeps its paper image origin (LAYOUT groups 148/149) through a DXF save, as
//! the file had it, so another program puts the sheet back where it was. A page CADaddy made
//! writes 0,0 as before and reads back as none. (The layout limits, groups 10/11, were already
//! kept: they follow the layout's sheet origin.)

use cadaddy_doc::geom::Vec2;
use cadaddy_doc::{Drawing, Layout};
use cadaddy_io::{read_dxf, write_dxf};

/// A drawing whose layout's page has an image origin away from 0,0.
fn drawing() -> Drawing {
    let mut d = Drawing::new_imperial();
    let mut layout = Layout::new("Sheet", 1);
    layout.page.image_origin = Some(Vec2::new(-1.054, -0.71));
    d.layouts = vec![layout];
    d
}

fn origin(d: &Drawing, how: &str) -> Vec2 {
    let l = d.layouts.iter().find(|l| l.name == "Sheet").expect("the layout");
    l.page.image_origin.unwrap_or_else(|| panic!("{how}: the image origin is kept"))
}

#[test]
fn a_layout_keeps_its_image_origin_through_dxf() {
    let back = read_dxf(write_dxf(&drawing()).as_bytes()).expect("reopen");
    assert!((origin(&back, "DXF") - Vec2::new(-1.054, -0.71)).len() < 1e-9);
    // And again: a second save writes what the first read.
    let again = read_dxf(write_dxf(&back).as_bytes()).expect("reopen");
    assert!((origin(&again, "DXF twice") - Vec2::new(-1.054, -0.71)).len() < 1e-9);
}

#[test]
fn a_new_layout_writes_a_zero_image_origin() {
    let text = write_dxf(&Drawing::new_imperial());
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    let after = |code: &str| lines.windows(2).filter(|w| w[0] == code).map(|w| w[1]).collect::<Vec<_>>();
    assert!(after("148").iter().all(|v| v.parse::<f64>() == Ok(0.0)), "{:?}", after("148"));
    let back = read_dxf(text.as_bytes()).expect("reopen");
    assert!(back.layouts.iter().all(|l| l.page.image_origin.is_none()));
}

#[test]
fn a_hostile_image_origin_is_not_kept() {
    let mut text = write_dxf(&drawing());
    // Swap the written origin for one past the range a page can have.
    let n = text.matches("-1.054").count();
    assert!(n > 0, "the origin is written");
    text = text.replace("-1.054", "1e300");
    let back = read_dxf(text.as_bytes()).expect("reopen");
    assert!(back.layouts.iter().all(|l| l.page.image_origin.is_none_or(|o| o.x.abs() < 1e12)));
}
