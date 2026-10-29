//! Battle test 4 message and limit cleanups (B4-023, 024, 026, 030, 033, 034).

use cadaddy_engine::Session;
use serde_json::json;

fn hooks() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
        read_dwf: None,
        write_dwfx: None,
    });
    cadaddy_engine::cmd::file::set_write_as(|d, n, o| {
        cadaddy_io::write_with(d, n, &cadaddy_io::SaveOptions { version: o.version, binary: o.binary }).map_err(|e| e.to_string())
    });
}

/// B4-023: commands that only took JSON parameters ask for them when typed bare.
#[test]
fn bare_typed_commands_prompt_instead_of_refusing() {
    let ids = cadaddy_engine::cmd::typed_ask::asking_commands();
    assert!(ids.len() >= 22, "only {} commands ask: {ids:?}", ids.len());
    for id in ids {
        let mut s = Session::new();
        s.cmdline(id).unwrap_or_else(|e| panic!("{id} typed bare: {e}"));
        assert!(s.running.is_some(), "{id}: no prompt, history {:?}", s.log.last());
        let prompt = s.prompt_text();
        assert!(!prompt.contains("invalid parameters") && (prompt.contains("Enter") || prompt.contains("Specify")), "{id}: {prompt}");
    }
    let mut s = Session::new();
    s.cmdline("delay").unwrap();
    assert!(s.prompt_text().contains("Enter delay time (in milliseconds):"), "{}", s.prompt_text());
    s.cancel();
    s.cmdline("ucsicon").unwrap();
    assert!(s.prompt_text().contains("Enter an option [ON/OFF/Origin/Noorigin] <ON>:"), "{}", s.prompt_text());
    s.cmdline("off").unwrap();
    assert!(s.running.is_none());
    // The answer runs the command with it.
    s.cmdline("delay").unwrap();
    s.cmdline("5").unwrap();
    assert!(s.running.is_none());
}

/// B4-024: draw and edit commands refuse a result beyond the ±1e12 export limit with one message.
#[test]
fn results_beyond_the_export_limit_are_refused() {
    let mut s = Session::new();
    for (cmd, p) in [
        ("circle", json!({ "center": [2e12, 0], "radius": 5 })),
        ("circle", json!({ "center": [0, 0], "radius": 5e12 })),
        ("line", json!({ "points": [[0, 0], [5e12, 5]] })),
    ] {
        let e = s.execute(cmd, &p).expect_err(&format!("{cmd} {p} must be refused")).to_string();
        assert!(e.contains("too large") && e.contains("1e12"), "{cmd}: {e}");
    }
    assert_eq!(s.doc().unwrap().model.len(), 0, "a refused command leaves nothing behind");
    s.execute("circle", &json!({ "center": [0, 0], "radius": 5 })).unwrap();
    let h = s.doc().unwrap().model.iter().next().unwrap().handle.hex();
    for (cmd, p) in [
        ("move", json!({ "handles": [h], "from": [0, 0], "to": [3e12, 0] })),
        ("move", json!({ "handles": [h], "delta": [0, -3e12] })),
        ("scale", json!({ "handles": [h], "base": [0, 0], "factor": 1e13 })),
    ] {
        let e = s.execute(cmd, &p).expect_err(&format!("{cmd} {p} must be refused")).to_string();
        assert!(e.contains("too large") && e.contains("1e12"), "{cmd}: {e}");
    }
    assert_eq!(s.doc().unwrap().model.len(), 1);
    // Inside the limit is fine.
    s.execute("move", &json!({ "handles": [h], "delta": [9e11, 0] })).unwrap();
}

/// A drawing from a file can hold an object beyond the limit; it can still be edited back in.
#[test]
fn an_object_already_beyond_the_limit_can_be_moved_back() {
    use cadaddy_engine::doc::{Circle, Common, EntityKind, Space};
    let mut s = Session::new();
    let circle = Circle { center: cadaddy_engine::geom::Vec3::new(5e12, 0.0, 0.0), radius: 1.0 };
    let st = s.state_mut().unwrap();
    let h = std::sync::Arc::make_mut(&mut st.doc).add(&Space::Model, Common::default(), EntityKind::Circle(circle)).unwrap().hex();
    s.execute("move", &json!({ "handles": [h], "delta": [-4e12, 0] })).unwrap();
    s.execute("move", &json!({ "handles": [h], "delta": [-9.9e11, 0] })).unwrap();
}

/// B4-026: RENDER and model.render take `path` and `output` alike, and both name what they take.
#[test]
fn render_paths_and_unknown_parameters() {
    let mut s = Session::new();
    for id in ["render", "rendercrop", "model.render", "model.overlay"] {
        let doc = cadaddy_engine::find_command(id).unwrap().params;
        assert!(doc.contains("path?") && doc.contains("output?"), "{id}: {doc}");
    }
    let e = s.execute_agent("render", &json!({ "nosuchparam": "x.png" })).unwrap_err().to_string();
    assert!(e.contains("`nosuchparam` is not a parameter of render"), "{e}");
}

/// B4-033: SETVAR refuses values outside AutoCAD's range for the variable.
#[test]
fn setvar_checks_ranges() {
    let mut s = Session::new();
    let bad = [
        ("FILLETRAD", json!(-1)),
        ("FILLETRAD", json!(1e15)),
        ("CHAMFERA", json!(-0.5)),
        ("AUPREC", json!(9)),
        ("LUPREC", json!(-1)),
        ("LUNITS", json!(0)),
        ("LUNITS", json!(6)),
        ("DIMTAD", json!(5)),
        ("DIMSCALE", json!(-1)),
        ("DIMASZ", json!(-1)),
        ("LTSCALE", json!(0)),
        ("TEXTSIZE", json!(-2)),
        ("MIRRTEXT", json!(2)),
        ("PDMODE", json!(5)),
        ("PICKBOX", json!(51)),
        ("GRIPSIZE", json!(0)),
        ("USERI1", json!(40000)),
        ("DIMDEC", json!("9")),
    ];
    for (name, v) in bad {
        let e = s.execute("setvar", &json!({ "name": name, "value": v })).expect_err(&format!("{name} = {v} must be refused")).to_string();
        assert!(e.contains(name), "{name}: {e}");
    }
    let before = cadaddy_engine::sysvars::get(&s, "FILLETRAD");
    let _ = s.execute("setvar", &json!({ "name": "FILLETRAD", "value": -3 }));
    assert_eq!(cadaddy_engine::sysvars::get(&s, "FILLETRAD"), before, "a refused value is not stored");
    for (name, v) in
        [("FILLETRAD", json!(2.5)), ("AUPREC", json!(8)), ("PDMODE", json!(35)), ("DIMTAD", json!(4)), ("USERI1", json!(-5)), ("LTSCALE", json!(0.5))]
    {
        s.execute("setvar", &json!({ "name": name, "value": v })).unwrap_or_else(|e| panic!("{name} = {v}: {e}"));
    }
}

/// B4-034: errors and the history echo long input shortened.
#[test]
fn long_input_is_clipped_in_errors_and_history() {
    let mut s = Session::new();
    let line = "x".repeat(2_000_000);
    let e = s.cmdline(&line).unwrap_err().to_string();
    assert!(e.len() < 300 && e.contains("(2000000 chars)"), "{} bytes: {}", e.len(), &e[..e.len().min(200)]);
    let echoed: usize = s.log.iter().map(String::len).sum();
    assert!(echoed < 1000, "history holds {echoed} bytes");
    assert_eq!(cadaddy_engine::clip_input("short"), "short");
}

/// B4-030: a file name of 255 bytes saves (and saves again); a longer one is refused in words.
#[test]
fn long_file_names_save_or_say_why_not() {
    let dir = std::env::temp_dir().join(format!("cadaddy-longname-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    hooks();
    let mut s = Session::new();
    s.execute("circle", &json!({ "center": [0, 0], "radius": 5 })).unwrap();
    for ext in ["cdy", "dxf", "dwg"] {
        // 255 bytes: multi-byte letters up front, the rest plain.
        let name = format!("{}{}.{ext}", "é".repeat(50), "a".repeat(255 - 100 - 1 - ext.len()));
        assert_eq!(name.len(), 255);
        let path = dir.join(&name);
        s.execute("saveas", &json!({ "path": path.to_string_lossy() })).unwrap_or_else(|e| panic!("{} bytes: {e}", name.len()));
        s.execute("saveas", &json!({ "path": path.to_string_lossy() })).unwrap_or_else(|e| panic!("again, {} bytes: {e}", name.len()));
    }
    let long = dir.join(format!("{}.dwg", "a".repeat(260)));
    let e = s.execute("saveas", &json!({ "path": long.to_string_lossy() })).unwrap_err().to_string();
    assert!(e.contains("file name is too long") && e.contains("255") && !e.contains("os error"), "{e}");
    std::fs::remove_dir_all(&dir).ok();
}
