//! Q6011: every command's JSON output must be accepted by a strict JSON parser — no raw
//! control characters.

use serde_json::{Value, json};

/// Assert that `s` contains no raw C0 control characters (0x00–0x1F) except the three JSON
/// permits inside strings when escaped: TAB (0x09), LF (0x0A), CR (0x0D).  This checks the
/// *serialised bytes*, not the Value tree: a string `"\u001b"` in JSON is six visible ASCII
/// characters and is fine; a literal ESC byte is not.
fn assert_strict_json(s: &str, context: &str) {
    for (i, b) in s.bytes().enumerate() {
        if b < 0x20 && b != b'\t' && b != b'\n' && b != b'\r' {
            panic!("raw control character 0x{b:02x} at byte {i} in {context}: …{}…", &s[i.saturating_sub(20)..(i + 20).min(s.len())]);
        }
    }
    // Also verify the string round-trips through a strict parser.
    assert!(serde_json::from_str::<Value>(s).is_ok(), "serde_json rejects the output of {context}");
}

fn session() -> cadaddy_engine::Session {
    let mut s = cadaddy_engine::Session::for_app();
    s.new_drawing_with_viewports(false);
    s
}

/// Inject text entities containing every C0 control character and run commands that read them
/// back. The JSON output must never contain a raw control character.
#[test]
fn command_output_has_no_raw_control_characters() {
    let mut s = session();
    // Plant a text entity whose content has every control character 0x00–0x1F.
    let dirty: String = (0u8..0x20).map(|b| b as char).collect();
    s.execute("text", &json!({"text": dirty, "at": [10.0, 20.0], "height": 5.0})).expect("text");
    // Run commands that would include the dirty text in their output.
    for cmd in ["drawing.inspect", "drawing.summary"] {
        let r = s.execute(cmd, &json!({})).expect(cmd);
        let serialised = serde_json::to_string(&r).expect("serialise");
        assert_strict_json(&serialised, cmd);
    }
}

/// The echo log (which feeds `cmdline.input`'s `output` and `cmdline.state`'s `history`)
/// never contains raw control characters, even when an echoed message originates from text
/// with control characters.
#[test]
fn echo_strips_control_characters() {
    let mut s = session();
    let dirty = "info\x1b[31mred\x1b[0m\x07bell\x00null".to_string();
    s.echo(dirty);
    let last = s.log.last().expect("log entry");
    for b in last.bytes() {
        assert!(b >= 0x20 || b == b'\t' || b == b'\n' || b == b'\r', "raw 0x{b:02x} in echo log: {last:?}");
    }
}

/// The sanitize function cleans a Value tree of control characters.
#[test]
fn sanitize_cleans_nested_values() {
    let mut v = json!({
        "a": "x\x00y".to_string(),
        "b": ["\x1b[1m".to_string(), "clean"],
        "c": {"d": "\x07\x08".to_string()}
    });
    cadaddy_engine::json_clean::sanitize(&mut v);
    let s = serde_json::to_string(&v).expect("serialise");
    assert_strict_json(&s, "sanitized value");
    // The replacement character is U+FFFD.
    assert!(v["a"].as_str().unwrap().contains('\u{FFFD}'));
    assert!(v["b"][1].as_str().unwrap() == "clean");
}

/// Every command that takes no required parameters runs without crashing and produces strict JSON.
#[test]
fn parameterless_commands_produce_strict_json() {
    let mut s = session();
    // Draw a simple shape so queries have something to return.
    s.execute("circle", &json!({"center": [0.0, 0.0], "radius": 5.0})).ok();
    let specs = cadaddy_engine::command_specs();
    let skip = [
        // These need an open file, specific state, or have side effects.
        "close",
        "closeall",
        "quit",
        "qsave",
        "saveas",
        "save",
        "new",
        "open",
        "recover",
        "plot",
        "publish",
        "plotstamp",
        "preview",
        // These need user interaction or specific UI state.
        "select",
        "selectall",
    ];
    let mut tested = 0;
    for spec in specs {
        if skip.iter().any(|s| spec.id.contains(s)) {
            continue;
        }
        // Only test commands that take no required parameters (the empty-params call).
        if let Ok(r) = s.execute(spec.id, &json!({})) {
            let serialised = serde_json::to_string(&r).expect("serialise");
            assert_strict_json(&serialised, spec.id);
            tested += 1;
        }
        // Commands that fail with BadParams are fine: they need arguments.
    }
    assert!(tested > 10, "expected to test more than 10 commands, got {tested}");
}
