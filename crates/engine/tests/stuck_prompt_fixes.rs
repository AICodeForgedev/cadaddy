//! The prompts the stuck prompts sweep (`stuck_prompts.rs`) found trapping the command line: each
//! now ends the command or says why it asks again, and the next typed command starts.

use cadaddy_engine::doc::Handle;
use cadaddy_engine::geom::Vec2;
use cadaddy_engine::{Input, Session};
use serde_json::json;

fn handle(r: &serde_json::Value) -> Handle {
    Handle::parse_hex(r["handle"].as_str().or_else(|| r["handles"][0].as_str()).unwrap()).unwrap()
}

fn line(s: &mut Session) -> Handle {
    handle(&s.execute("line", &json!({"points": [[0, 0], [10, 0]]})).unwrap())
}

fn said(s: &Session, since: usize) -> String {
    s.log[since..].join(" | ")
}

/// The command has ended and a typed LINE starts.
fn line_starts(s: &mut Session) {
    assert!(s.running.is_none(), "still running: {}", s.prompt_text());
    s.cmdline("line").unwrap();
    assert_eq!(s.running.as_ref().map(|r| r.id.as_str()), Some("line"));
    s.cancel();
}

#[test]
fn enter_the_command_refuses_ends_it() {
    // UNION of lines: Enter runs it, it can't, and Enter again used to fail the same way forever.
    let mut s = Session::new();
    let a = line(&mut s);
    s.cmdline("union").unwrap();
    s.input(Input::Pick(vec![a])).unwrap();
    let n = s.log.len();
    s.input(Input::Enter).unwrap();
    assert!(said(&s, n).contains("Select at least one region."), "{}", said(&s, n));
    line_starts(&mut s);
    // EXTRUDE of an open line: the height can't help.
    let mut s = Session::new();
    let a = line(&mut s);
    s.cmdline("extrude").unwrap();
    s.input(Input::Pick(vec![a])).unwrap();
    s.input(Input::Enter).unwrap();
    s.input(Input::Enter).unwrap();
    line_starts(&mut s);
}

#[test]
fn text_a_prompt_cant_take_is_answered() {
    let mut s = Session::new();
    line(&mut s);
    s.cmdline("-block").unwrap();
    s.cmdline("B").unwrap();
    let n = s.log.len();
    s.cmdline("qq#7?").unwrap();
    assert_eq!(said(&s, n), "Point or option keyword required.");
    assert!(s.running.is_some(), "asked again");
    s.cancel();
}

#[test]
fn enter_at_a_name_or_file_prompt_ends_the_command() {
    for cmd in ["insert", "xattach", "pdfattach", "geographiclocation"] {
        let mut s = Session::new();
        s.cmdline(cmd).unwrap();
        s.input(Input::Enter).unwrap();
        line_starts(&mut s);
    }
    // INSERT: a typed name that isn't a block is asked again, with the reason.
    let mut s = Session::new();
    s.cmdline("insert").unwrap();
    let n = s.log.len();
    s.cmdline("Nope").unwrap();
    assert!(said(&s, n).contains("Block \"Nope\" not found."));
    assert!(s.running.is_some());
    s.cancel();
}

#[test]
fn views_with_nothing_to_draw_end_the_command() {
    for cmd in ["flatshot", "viewbase"] {
        let mut s = Session::new();
        line(&mut s);
        s.cmdline(cmd).unwrap();
        let n = s.log.len();
        s.input(Input::Point(Vec2::new(50.0, 50.0))).unwrap();
        assert!(said(&s, n).contains("nothing to draw"), "{cmd}: {}", said(&s, n));
        line_starts(&mut s);
        s.cmdline(cmd).unwrap();
        s.input(Input::Enter).unwrap();
        line_starts(&mut s);
    }
}

#[test]
fn subtract_that_fails_ends_instead_of_starting_over() {
    let mut s = Session::new();
    let a = line(&mut s);
    let b = line(&mut s);
    s.cmdline("subtract").unwrap();
    s.input(Input::Pick(vec![a])).unwrap();
    s.input(Input::Enter).unwrap();
    s.input(Input::Pick(vec![b])).unwrap();
    s.input(Input::Enter).unwrap();
    line_starts(&mut s);
}

#[test]
fn enter_at_an_object_prompt_ends_the_command() {
    // TEXTFIT, MLEADERALIGN (its "align to"), DIMLINEAR's "Select object to dimension".
    let mut s = Session::new();
    s.cmdline("textfit").unwrap();
    s.input(Input::Enter).unwrap();
    line_starts(&mut s);

    let mut s = Session::new();
    s.cmdline("dimlinear").unwrap();
    s.input(Input::Enter).unwrap();
    assert_eq!(s.current_prompt().unwrap().message, "Select object to dimension");
    s.input(Input::Enter).unwrap();
    line_starts(&mut s);

    let mut s = Session::new();
    let m = handle(&s.execute("mleader", &json!({"points": [[0, 0], [5, 5]], "text": "A"})).unwrap());
    s.cmdline("mleaderalign").unwrap();
    s.input(Input::Pick(vec![m])).unwrap();
    s.input(Input::Enter).unwrap();
    s.input(Input::Enter).unwrap();
    line_starts(&mut s);
}

#[test]
fn mleadercollect_and_rendercrop_end_when_they_cant() {
    let mut s = Session::new();
    let m = handle(&s.execute("mleader", &json!({"points": [[0, 0], [5, 5]], "text": "A"})).unwrap());
    s.cmdline("mleadercollect").unwrap();
    s.input(Input::Pick(vec![m])).unwrap();
    s.input(Input::Enter).unwrap();
    let n = s.log.len();
    s.input(Input::Point(Vec2::new(20.0, 20.0))).unwrap();
    assert!(said(&s, n).contains("Need two or more multileaders"), "{}", said(&s, n));
    line_starts(&mut s);

    let mut s = Session::new();
    line(&mut s);
    s.cmdline("rendercrop").unwrap();
    s.input(Input::Point(Vec2::new(0.0, 0.0))).unwrap();
    s.input(Input::Enter).unwrap();
    line_starts(&mut s);
    s.cmdline("rendercrop").unwrap();
    s.input(Input::Point(Vec2::new(0.0, 0.0))).unwrap();
    s.input(Input::Point(Vec2::new(5.0, 5.0))).unwrap();
    line_starts(&mut s);
}

#[test]
fn keyword_prompts_say_when_a_word_isnt_one() {
    let mut s = Session::new();
    s.cmdline("hatchpick").unwrap();
    let n = s.log.len();
    s.cmdline("qq").unwrap();
    assert_eq!(said(&s, n), "Invalid option keyword.");
    s.cancel();

    let mut s = Session::new();
    let d = handle(&s.execute("dimlinear", &json!({"p1": [0, 0], "p2": [10, 0], "at": [5, -2]})).unwrap());
    s.cmdline("diminspect").unwrap();
    s.input(Input::Pick(vec![d])).unwrap();
    s.input(Input::Enter).unwrap();
    let n = s.log.len();
    s.cmdline("qq").unwrap();
    assert_eq!(said(&s, n), "Invalid option keyword.");
    s.cancel();
}
