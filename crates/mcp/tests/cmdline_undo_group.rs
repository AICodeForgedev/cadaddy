//! One typed command is one undo step on the headless command-line path, as in the app: a single
//! UNDO returns the drawing to exactly what it was before the command, also when Esc ended it.

use cadaddy_mcp::{Backend, Headless};
use serde_json::json;

/// The drawing's entities as text (handles, kinds and geometry), for an exact before/after check.
fn fingerprint(b: &mut Headless) -> String {
    b.call("drawing.inspect", json!({})).unwrap().to_string()
}

fn type_line(b: &mut Headless, l: &str) {
    let r = b.call("cmdline.input", json!({"text": l})).unwrap();
    assert!(r.get("error").is_none(), "{l}: {r}");
}

fn seeded() -> Headless {
    let mut b = Headless::default();
    b.call("engine.execute", json!({"command": "new", "params": {}})).unwrap();
    for l in ["line 0,0 50,0 ", "circle 20,20 5"] {
        type_line(&mut b, l);
    }
    b
}

fn undo(b: &mut Headless) {
    b.call("engine.execute", json!({"command": "undo", "params": {"count": 1}})).unwrap();
}

/// Types `lines`, then (when `esc`) Esc, and expects one UNDO to restore the starting drawing.
fn one_step(name: &str, lines: &[&str], esc: bool) {
    let mut b = seeded();
    let before = fingerprint(&mut b);
    for l in lines {
        type_line(&mut b, l);
    }
    if esc {
        b.call("cmdline.key", json!({"key": "escape"})).unwrap();
    }
    let drawn = fingerprint(&mut b);
    undo(&mut b);
    assert_eq!(fingerprint(&mut b), before, "{name}: one UNDO must restore the drawing (drawn: {})", drawn == before);
    assert_ne!(drawn, before, "{name}: the command drew nothing, test is vacuous");
}

#[test]
fn line_is_one_step() {
    one_step("line", &["line", "0,0", "10,0", "10,10", ""], false);
    one_step("line esc", &["line", "0,0", "10,0", "10,10"], true);
}

#[test]
fn pline_is_one_step() {
    one_step("pline", &["pline", "0,0", "10,0", "10,10", ""], false);
    one_step("pline esc", &["pline", "0,0", "10,0", "10,10"], true);
}

#[test]
fn spline_is_one_step() {
    one_step("spline", &["spline", "0,0", "10,10", "20,0", "30,10", "", "", ""], false);
}

#[test]
fn arc_is_one_step() {
    one_step("arc", &["arc", "0,0", "5,5", "10,0"], false);
}

#[test]
fn mline_is_one_step() {
    one_step("mline", &["mline", "0,0", "10,0", "10,10", ""], false);
}

#[test]
fn measure_and_divide_are_one_step() {
    one_step("divide", &["divide", "25,0", "4"], false);
    one_step("measure", &["measure", "25,0", "10"], false);
}

/// The tool layer: the server reads `document.list` after every call, which used to split the
/// running command's undo step at each input (one UNDO took back one segment).
mod through_tools {
    use cadaddy_mcp::{Headless, Server};
    use serde_json::{Value, json};

    fn tool(s: &mut Server, name: &str, args: Value) -> Value {
        let line = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": name, "arguments": args}}).to_string();
        let r: Value = serde_json::from_str(&s.handle_line(&line).expect("reply")).expect("json");
        assert_ne!(r["result"]["isError"], json!(true), "{name}: {r}");
        serde_json::from_str(r["result"]["content"][0]["text"].as_str().unwrap_or("null")).unwrap_or(Value::Null)
    }

    fn count(s: &mut Server) -> u64 {
        tool(s, "inspect_drawing", json!({}))["entityCount"].as_u64().unwrap_or(u64::MAX)
    }

    /// Types `lines` (then Esc), checks the drawing grew, and that one UNDO restores the start.
    fn one_undo(name: &str, lines: &[&str], esc: bool) {
        let mut s = Server::new(Box::new(Headless::default()));
        tool(&mut s, "new_drawing", json!({}));
        let n0 = count(&mut s);
        for l in lines {
            tool(&mut s, "command_line", json!({"text": l}));
            // Reads between the inputs, as an agent makes them.
            tool(&mut s, "query_entities", json!({}));
        }
        if esc {
            tool(&mut s, "cancel", json!({}));
        }
        assert!(count(&mut s) > n0, "{name}: nothing was drawn");
        tool(&mut s, "undo", json!({}));
        assert_eq!(count(&mut s), n0, "{name}: one UNDO must take back the whole command");
    }

    #[test]
    fn line_and_pline_typed_through_the_tools_undo_in_one_step() {
        one_undo("line", &["line", "0,0", "10,0", "10,10", ""], false);
        one_undo("pline", &["pline", "0,0", "10,0", "10,10", ""], false);
        one_undo("line esc", &["line", "0,0", "10,0", "10,10"], true);
        one_undo("pline esc", &["pline", "0,0", "10,0", "10,10"], true);
        one_undo("mline", &["mline", "0,0", "10,0", "10,10", ""], false);
        one_undo("spline", &["spline", "0,0", "10,10", "20,0", "30,10", "", "", ""], false);
        one_undo("arc", &["arc", "0,0", "5,5", "10,0"], false);
    }

    #[test]
    fn pline_undo_option_still_works_inside_the_command() {
        let mut s = Server::new(Box::new(Headless::default()));
        tool(&mut s, "new_drawing", json!({}));
        let n0 = count(&mut s);
        for l in ["pline", "0,0", "10,0", "10,10", "u", "20,0", ""] {
            tool(&mut s, "command_line", json!({"text": l}));
        }
        assert!(count(&mut s) > n0);
        tool(&mut s, "undo", json!({}));
        assert_eq!(count(&mut s), n0);
    }

    #[test]
    fn a_call_that_edits_mid_command_is_still_its_own_step() {
        let mut s = Server::new(Box::new(Headless::default()));
        tool(&mut s, "new_drawing", json!({}));
        let n0 = count(&mut s);
        for l in ["line", "0,0", "10,0"] {
            tool(&mut s, "command_line", json!({"text": l}));
        }
        tool(&mut s, "execute", json!({"command": "layer.new", "params": {"name": "Notes"}}));
        for l in ["10,10", ""] {
            tool(&mut s, "command_line", json!({"text": l}));
        }
        let n1 = count(&mut s);
        assert!(n1 > n0);
        tool(&mut s, "undo", json!({}));
        assert!(count(&mut s) < n1, "the command's rest is a step of its own");
    }
}
