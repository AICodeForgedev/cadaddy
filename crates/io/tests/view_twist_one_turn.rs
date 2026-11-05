//! A view twist is read and written within one turn: a hostile twist set from JSON or MCP writes
//! a sane angle, and a tiny negative twist in a file reads as none, not as a full turn.

use cadaddy_doc::Drawing;
use cadaddy_doc::NamedView;
use cadaddy_io::{read_dxf, write_dxf};

fn view(twist: f64) -> Drawing {
    let mut d = Drawing::new_imperial();
    d.views.push(NamedView { name: "Turned".into(), height: 10.0, width: 10.0, twist, ..NamedView::default() });
    d
}

/// The value of group `code` in the VIEW record named `name`.
fn view_group(text: &str, name: &str, code: i32) -> Option<f64> {
    let tags = cadaddy_dxf::parse(text.as_bytes()).ok()?;
    let mut records = tags.split(|t| t.code == 0);
    records.find(|r| r.iter().any(|t| t.code == 2 && t.str() == name) && r.iter().any(|t| t.code == 50))?.iter().find(|t| t.code == code).map(|t| t.value.as_f64())
}

#[test]
fn a_huge_twist_is_written_within_one_turn() {
    for twist in [1e300, -1e300, 7.0 * std::f64::consts::TAU + 0.5, f64::NAN] {
        let text = write_dxf(&view(twist));
        let deg = view_group(&text, "Turned", 50).expect("the twist is written");
        assert!((0.0..360.0).contains(&deg), "{twist} written as {deg}");
    }
}

#[test]
fn a_tiny_negative_twist_reads_as_none() {
    let text = write_dxf(&view(0.25));
    let written = format!("{}", 0.25f64.to_degrees());
    assert!(text.contains(&written), "the twist is written as {written}");
    let text = text.replace(&written, "-1e-20");
    let back = read_dxf(text.as_bytes()).expect("reopen");
    let v = back.views.iter().find(|v| v.name == "Turned").expect("the view");
    assert!(v.twist < 1e-9, "read as {}", v.twist);
}
