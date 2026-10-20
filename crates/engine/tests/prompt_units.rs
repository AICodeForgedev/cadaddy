//! Prompt defaults and messages show lengths in the drawing's units (battle test 0.7.8 #13:
//! `OFFSET … <6.0000>` and `Text height: 0.2000` in an architectural drawing).

use cadaddy_engine::Session;
use cadaddy_engine::units::format_distance;
use serde_json::json;

fn arch() -> Session {
    let mut s = Session::new();
    s.execute("setvar", &json!({"name": "LUNITS", "value": 4})).unwrap();
    s
}

fn default(s: &Session) -> String {
    s.current_prompt().and_then(|p| p.default).unwrap_or_default()
}

fn feet_inches(v: f64) -> String {
    format_distance(v, 4, 4)
}

#[test]
fn defaults_are_in_feet_and_inches() {
    let mut s = arch();
    s.execute("setvar", &json!({"name": "OFFSETDIST", "value": 6})).unwrap();
    s.cmdline("offset").unwrap();
    assert_eq!(default(&s), feet_inches(6.0));
    s.cancel();

    s.cmdline("circle").unwrap();
    s.cmdline("0,0").unwrap();
    s.cmdline("48").unwrap();
    s.cmdline("circle").unwrap();
    s.cmdline("100,0").unwrap();
    assert_eq!(default(&s), feet_inches(48.0));
    assert_eq!(default(&s), "4'-0\"");
    s.cancel();

    s.execute("setvar", &json!({"name": "FILLETRAD", "value": 3})).unwrap();
    s.cmdline("fillet").unwrap();
    s.cmdline("r").unwrap();
    assert_eq!(default(&s), feet_inches(3.0));
    s.cancel();
}

#[test]
fn messages_are_in_feet_and_inches() {
    let mut s = arch();
    s.execute("setvar", &json!({"name": "TEXTSIZE", "value": 4.5})).unwrap();
    let before = s.log.len();
    s.cmdline("text").unwrap();
    let said = s.log[before..].join("\n");
    assert!(said.contains(&format!("Text height:  {}", feet_inches(4.5))), "{said}");
    s.cancel();

    let before = s.log.len();
    s.cmdline("pline").unwrap();
    s.cmdline("0,0").unwrap();
    let said = s.log[before..].join("\n");
    assert!(said.contains(&format!("Current line-width is {}", feet_inches(0.0))), "{said}");
    s.cancel();
}

#[test]
fn decimal_drawings_keep_four_places() {
    let mut s = Session::new();
    s.execute("setvar", &json!({"name": "OFFSETDIST", "value": 6})).unwrap();
    s.cmdline("offset").unwrap();
    assert_eq!(default(&s), "6.0000");
}
