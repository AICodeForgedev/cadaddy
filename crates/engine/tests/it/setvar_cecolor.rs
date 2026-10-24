//! `setvar CECOLOR` takes what the `color` command takes: "ByLayer", "ByBlock", a colour name or an
//! index (#1999).

use cadcraft_engine::Session;
use cadcraft_engine::doc::color::{Color, Rgb};
use serde_json::{Value, json};

fn cecolor(s: &mut Session) -> Value {
    s.execute("getvar", &json!({ "name": "CECOLOR" })).unwrap()["value"].clone()
}

fn new_line_color(s: &mut Session) -> Color {
    s.execute("line", &json!({ "points": [[0, 0], [10, 0]] })).unwrap();
    s.doc().unwrap().model.last().unwrap().common.color
}

#[test]
fn setvar_cecolor_accepts_names() {
    for (value, aci) in [("BYLAYER", 256), ("ByLayer", 256), ("BYBLOCK", 0), ("red", 1), ("250", 250)] {
        let mut s = Session::new();
        s.execute("setvar", &json!({ "name": "CECOLOR", "value": value })).unwrap_or_else(|e| panic!("{value}: {e}"));
        assert_eq!(cecolor(&mut s), json!(aci), "{value}");
    }
}

#[test]
fn setvar_cecolor_name_replaces_a_true_colour() {
    let mut s = Session::new();
    s.execute("color", &json!({ "color": "10,20,30" })).unwrap();
    assert_eq!(new_line_color(&mut s), Color::True(Rgb(10, 20, 30)));
    s.execute("setvar", &json!({ "name": "CECOLOR", "value": "ByLayer" })).unwrap();
    assert_eq!(cecolor(&mut s), json!(256));
    assert_eq!(new_line_color(&mut s), Color::ByLayer);
}

#[test]
fn setvar_cecolor_number_replaces_a_true_colour() {
    let mut s = Session::new();
    s.execute("color", &json!({ "color": "10,20,30" })).unwrap();
    s.execute("setvar", &json!({ "name": "CECOLOR", "value": 250 })).unwrap();
    assert_eq!(new_line_color(&mut s), Color::Index(250));
}

#[test]
fn setvar_cecolor_refuses_what_is_not_a_colour() {
    let mut s = Session::new();
    for bad in [json!("chartreuse"), json!(257), json!(-1), json!(true)] {
        assert!(s.execute("setvar", &json!({ "name": "CECOLOR", "value": bad })).is_err(), "{bad}");
    }
    assert_eq!(cecolor(&mut s), json!(256));
}
