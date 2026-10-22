//! The Rhai scripting API end to end: objects read and written, events, Stop and the time
//! budget, nested runs, the examples and the SECURELOAD rules.

use std::sync::Arc;

use cadaddy_doc::{EntityKind, Handle};
use serde_json::{Value, json};
use web_time::{Duration, Instant};

use crate::Session;

fn session() -> Session {
    let mut s = Session::new();
    s.execute("new", &json!({})).unwrap();
    s
}

/// Run `code` with `rhai` and return its value (panics on a script error).
fn eval(s: &mut Session, code: &str) -> Value {
    let r = s.execute("rhai", &json!({ "code": code })).unwrap();
    assert!(r.get("error").is_none(), "script error: {r}");
    r["value"].clone()
}

fn line_ends(s: &Session, h: &str) -> ([f64; 2], [f64; 2], String) {
    let d = s.doc().unwrap();
    let e = d.entity(Handle::parse_hex(h).unwrap()).unwrap();
    let EntityKind::Line(l) = &e.kind else { panic!("{h} is not a line: {:?}", e.kind) };
    ([l.a.x, l.a.y], [l.b.x, l.b.y], e.common.layer.to_string())
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9
}

/// Objects added to the drawing by `f`, in handle order.
fn added(s: &mut Session, f: impl FnOnce(&mut Session)) -> Vec<Handle> {
    let before = Arc::clone(&s.state().unwrap().doc);
    f(s);
    let (mut a, _, _) = before.model.changes(&s.state().unwrap().doc.model);
    a.sort_unstable_by_key(|h| h.0);
    a
}

// ---- MUST 1: entity(handle) ----

#[test]
fn entity_reads_a_line_drawn_by_the_script() {
    let mut s = session();
    let v = eval(&mut s, r#"command("line", #{ points: [[0.0, 0.0], [10.0, 10.0]] }).handles[0]"#);
    let h = v.as_str().unwrap().to_string();
    s.set_selection(vec![Handle::parse_hex(&h).unwrap()]);
    let ent = eval(&mut s, "let h = selection()[0]; let e = entity(h); [h, e.type, e.geometry.a.x, e.geometry.a.y, e.geometry.b.x, e.geometry.b.y]");
    let a = ent.as_array().unwrap();
    assert_eq!(a[0], json!(h));
    assert_eq!(a[1], json!("Line"));
    for (got, want) in a[2..].iter().zip([0.0, 0.0, 10.0, 10.0]) {
        assert!(near(got.as_f64().unwrap(), want), "endpoint {got} != {want}: {ent}");
    }
}

#[test]
fn entity_of_an_unknown_handle_is_a_script_error() {
    let mut s = session();
    let r = s.execute("rhai", &json!({ "code": r#"entity("FFFFFF")"# })).unwrap();
    assert!(r["error"].as_str().unwrap().contains("no object with handle FFFFFF"), "{r}");
}

// ---- MUST 2: set_entity(handle, map) ----

#[test]
fn set_entity_changes_layer_and_end_point() {
    let mut s = session();
    s.execute("layer.new", &json!({ "name": "Walls" })).unwrap();
    let h = eval(&mut s, r#"command("line", #{ points: [[0.0, 0.0], [10.0, 10.0]] }).handles[0]"#).as_str().unwrap().to_string();
    let undo_before = s.state().unwrap().undo.len();
    eval(&mut s, &format!(r#"set_entity("{h}", #{{ layer: "Walls", end: [25.5, -3.25] }})"#));
    let (a, b, layer) = line_ends(&s, &h);
    assert_eq!(layer, "Walls");
    assert_eq!(a, [0.0, 0.0]);
    assert_eq!(b, [25.5, -3.25]);
    // One undo step for the change, and the script sees the new values through entity().
    assert_eq!(s.state().unwrap().undo.len(), undo_before + 1);
    let v = eval(&mut s, &format!(r#"let e = entity("{h}"); [e.layer, e.geometry.b.x, e.geometry.b.y]"#));
    assert_eq!(v, json!(["Walls", 25.5, -3.25]));
}

#[test]
fn set_entity_with_a_bad_value_is_a_script_error_and_changes_nothing() {
    let mut s = session();
    let h = eval(&mut s, r#"command("line", #{ points: [[0.0, 0.0], [10.0, 10.0]] }).handles[0]"#).as_str().unwrap().to_string();
    let r = s.execute("rhai", &json!({ "code": format!(r#"set_entity("{h}", #{{ transparency: 500 }})"#) })).unwrap();
    assert!(r.get("error").is_some(), "{r}");
    assert_eq!(line_ends(&s, &h), ([0.0, 0.0], [10.0, 10.0], "0".to_string()));
}

// ---- MUST 3: events ----

#[test]
fn command_ended_fires_once_per_command_with_the_new_handle() {
    let mut s = session();
    let v = eval(
        &mut s,
        r#"
        let fired = 0; let got = [];
        on("command_ended", |ev| { fired += 1; got.push([ev.command, ev.handles]); });
        let a = command("line", #{ points: [[0.0, 0.0], [1.0, 0.0]] }).handles[0];
        let b = command("circle", #{ center: [5.0, 5.0], radius: 2.0 }).handle;
        [fired, got, a, b]
        "#,
    );
    let (a, b) = (v[2].as_str().unwrap(), v[3].as_str().unwrap());
    assert_eq!(v[0], json!(2), "{v}");
    assert_eq!(v[1], json!([["line", [a]], ["circle", [b]]]));
    // Outside the script: an agent's (or the command line's) command fires the handler too, once.
    let h = s.execute("line", &json!({ "points": [[0, 0], [0, 7]] })).unwrap()["handles"][0].as_str().unwrap().to_string();
    let v = eval(&mut s, "[fired, got[got.len() - 1]]");
    assert_eq!(v, json!([3, ["line", [h]]]));
    // A command that changes nothing fires command_ended with no handles; off() stops it all.
    assert_eq!(eval(&mut s, r#"command("properties"); [fired, got[got.len() - 1][1]]"#), json!([4, []]));
    assert_eq!(eval(&mut s, "off()"), json!(1));
    s.execute("line", &json!({ "points": [[0, 0], [3, 3]] })).unwrap();
    assert_eq!(eval(&mut s, "fired"), json!(4));
    assert_eq!(s.rhai.handler_count(), 0);
}

#[test]
fn objects_changed_lists_what_set_entity_and_erase_touched() {
    let mut s = session();
    let v = eval(
        &mut s,
        r#"
        let seen = [];
        let a = command("line", #{ points: [[0.0, 0.0], [1.0, 0.0]] }).handles[0];
        on("objects_changed", |ev| seen.push([ev.command, ev.added, ev.changed, ev.removed]));
        set_entity(a, #{ end: [2.0, 0.0] });
        command("erase", #{ handles: [a] });
        [a, seen]
        "#,
    );
    let a = v[0].as_str().unwrap();
    assert_eq!(v[1], json!([["properties.set", [], [a], []], ["erase", [], [], [a]]]));
}

/// OPEN itself needs the file readers (not in this crate's tests): this is the call each of its
/// paths makes once the drawing is in (`cmd::file`), then the dispatch after the command.
#[test]
fn drawing_opened_fires_with_the_title() {
    let mut s = session();
    eval(&mut s, r#"let opened = []; on("drawing_opened", |ev| opened.push([ev.event, ev.title]));"#);
    let i = s.new_drawing(false);
    s.docs[i].title = "Sample Floor Plan.dxf".into();
    s.rhai_drawing_opened(i);
    s.rhai_dispatch_pending();
    assert_eq!(eval(&mut s, "opened"), json!([["drawing_opened", "Sample Floor Plan.dxf"]]));
    // Queued once, told once.
    s.rhai_dispatch_pending();
    assert_eq!(eval(&mut s, "opened.len()"), json!(1));
}

#[test]
fn a_handler_s_own_commands_set_off_no_events() {
    let mut s = session();
    let v = eval(
        &mut s,
        r#"
        let fired = 0;
        on("command_ended", |ev| { fired += 1; command("line", #{ points: [[9.0, 9.0], [9.0, 10.0]] }); });
        command("line", #{ points: [[0.0, 0.0], [1.0, 0.0]] });
        fired
        "#,
    );
    assert_eq!(v, json!(1));
    let lines = s.doc().unwrap().model.iter().filter(|e| matches!(e.kind, EntityKind::Line(_))).count();
    assert_eq!(lines, 2, "the script's line and the handler's");
}

#[test]
fn on_refuses_an_unknown_event() {
    let mut s = session();
    let r = s.execute("rhai", &json!({ "code": r#"on("saved", |ev| 1)"# })).unwrap();
    assert!(r["error"].as_str().unwrap().contains("unknown event \"saved\""), "{r}");
}

// ---- MUST 4: Stop and the time budget ----

#[test]
fn an_endless_loop_stops_within_the_budget() {
    let mut s = session();
    let t = Instant::now();
    let r = s.execute("rhai", &json!({ "code": "loop {}", "timeoutMs": 300 })).unwrap();
    let took = t.elapsed();
    // Pure computation reaches the operation limit first.
    assert_eq!(r["error"], json!("script stopped: it ran past the limit of 10000000 operations"));
    assert!(took < Duration::from_millis(300), "took {took:?}");
    // A loop of calls into the drawing reaches the wall-clock budget.
    let t = Instant::now();
    let r = s.execute("rhai", &json!({ "code": r#"loop { getvar("CLAYER"); }"#, "timeoutMs": 300 })).unwrap();
    let took = t.elapsed();
    assert_eq!(r["error"], json!("script stopped: it ran past its time budget of 300 ms (pass `timeoutMs` for a longer one)"));
    assert!(took >= Duration::from_millis(300) && took < Duration::from_secs(3), "took {took:?}");
    // The session runs scripts again.
    assert_eq!(eval(&mut s, "1 + 1"), json!(2));
}

#[test]
fn an_endless_loop_of_commands_stops_within_the_budget() {
    let mut s = session();
    let t = Instant::now();
    let r = s.execute("rhai", &json!({ "code": r#"loop { command("line", #{ points: [[0.0, 0.0], [1.0, 1.0]] }); }"#, "timeoutMs": 300 })).unwrap();
    let took = t.elapsed();
    assert!(r["error"].as_str().unwrap().contains("script stopped"), "{r}");
    assert!(took < Duration::from_secs(3), "took {took:?}");
    assert!(s.doc().unwrap().model.iter().count() > 0, "the lines before the stop stay");
}

#[test]
fn a_bad_budget_is_refused() {
    let mut s = session();
    assert!(s.execute("rhai", &json!({ "code": "1", "timeoutMs": 0 })).is_err());
    assert!(s.execute("rhai", &json!({ "code": "1", "timeoutMs": 600_001 })).is_err());
}

/// Pump a worker script until it ends (at most `limit`).
fn pump_until_done(s: &mut Session, limit: Duration) -> crate::rhai_engine::RunResult {
    let t = Instant::now();
    while s.rhai_pump(Duration::from_millis(5)) {
        assert!(t.elapsed() < limit, "the script didn't end within {limit:?}");
    }
    s.rhai_take_finished().expect("a finished result")
}

#[test]
fn stop_ends_a_worker_script_spinning_in_loop() {
    let mut s = session();
    // Calls into the drawing each turn: the operation limit is far off, only Stop ends it.
    s.rhai_start(r#"loop { getvar("CLAYER"); }"#).unwrap();
    assert!(s.rhai_running());
    // The session's thread stays free while it spins.
    let t = Instant::now();
    assert!(s.rhai_pump(Duration::from_millis(50)));
    assert!(t.elapsed() < Duration::from_millis(500));
    s.execute("line", &json!({ "points": [[0, 0], [1, 1]] })).unwrap();
    s.rhai_stop();
    let (e, _) = pump_until_done(&mut s, Duration::from_secs(2)).unwrap_err();
    assert_eq!(e, "script stopped by the user");
    assert!(!s.rhai_running());
}

#[test]
fn stop_ends_a_worker_script_running_commands_and_keeps_its_variables() {
    let mut s = session();
    eval(&mut s, "let kept = 41;");
    s.rhai_start(r#"kept += 1; let n = 0; loop { command("line", #{ points: [[0.0, 0.0], [1.0, 1.0]] }); n += 1; print(`line ${n}`); }"#).unwrap();
    let t = Instant::now();
    while t.elapsed() < Duration::from_millis(200) {
        s.rhai_pump(Duration::from_millis(10));
    }
    s.rhai_stop();
    let (e, output) = pump_until_done(&mut s, Duration::from_secs(2)).unwrap_err();
    assert_eq!(e, "script stopped by the user");
    let lines = s.doc().unwrap().model.iter().count();
    assert!(lines > 0, "the worker's calls ran on the session's thread");
    // Each line printed was one command served (the last may have been cut off by the stop).
    assert!(output.len() == lines || output.len() + 1 == lines, "{} printed, {lines} lines", output.len());
    assert_eq!(output.first().map(String::as_str), Some("line 1"));
    assert_eq!(eval(&mut s, "kept"), json!(42));
}

#[test]
fn a_worker_script_returns_its_value() {
    let mut s = session();
    s.rhai_start(r#"let h = command("line", #{ points: [[0.0, 0.0], [3.0, 4.0]] }).handles[0]; entity(h).length"#).unwrap();
    let (v, _) = pump_until_done(&mut s, Duration::from_secs(5)).unwrap();
    assert_eq!(v, json!(5.0));
}

// ---- MUST 5: no panic on nested evaluation ----

#[test]
fn rhai_inside_a_script_is_a_script_error_and_keeps_the_outer_inputs() {
    let mut s = session();
    let reports = crate::crash_report::list().len();
    let r = s
        .execute(
            "rhai",
            &json!({ "code": r#"let a = getstring(""); let e = ""; try { command("rhai", #{ code: "1" }); } catch (err) { e = err; } let b = getstring(""); [a, b, e]"#, "inputs": ["first", "second"] }),
        )
        .unwrap();
    let v = &r["value"];
    assert_eq!(v[0], json!("first"));
    assert_eq!(v[1], json!("second"), "the outer queue is intact");
    assert!(v[2].as_str().unwrap().contains("a Rhai script is running already"), "{r}");
    assert_eq!(crate::crash_report::list().len(), reports, "no crash report");
    // Uncaught, it ends the script with that error; rhairun the same.
    for code in [r#"command("rhai", #{ code: "1" })"#, r#"command("rhairun", #{ text: "1" })"#] {
        let r = s.execute("rhai", &json!({ "code": code })).unwrap();
        assert!(r["error"].as_str().unwrap().contains("rhai and rhairun can't be called from a script"), "{r}");
    }
    assert_eq!(eval(&mut s, "40 + 2"), json!(42));
}

#[test]
fn rhai_while_a_worker_script_runs_is_refused() {
    let mut s = session();
    s.rhai_start(r#"loop { getvar("CLAYER"); }"#).unwrap();
    assert!(s.execute("rhai", &json!({ "code": "1" })).is_err());
    assert!(s.rhai_start("1").is_err());
    s.rhai_stop();
    pump_until_done(&mut s, Duration::from_secs(2)).unwrap_err();
    assert_eq!(eval(&mut s, "1"), json!(1));
}

// ---- MUST 6: the examples ----

fn example(name: &str) -> String {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/rhai").join(name);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn run_example(s: &mut Session, name: &str) -> Vec<String> {
    let r = s.execute("rhairun", &json!({ "name": name, "text": example(name) })).unwrap();
    assert!(r.get("error").is_none(), "{name}: {r}");
    r["output"].as_array().unwrap().iter().map(|l| l.as_str().unwrap().to_string()).collect()
}

#[test]
fn every_example_is_tested() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/rhai");
    let mut names: Vec<String> = std::fs::read_dir(dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    assert_eq!(
        names,
        ["draw_grid.rhai", "draw_star.rhai", "hello.rhai", "list_layers.rhai", "selection_info.rhai"],
        "a new example needs its test here"
    );
}

#[test]
fn example_hello() {
    let mut s = session();
    assert_eq!(run_example(&mut s, "hello.rhai"), ["Hello from Rhai!"]);
}

#[test]
fn example_draw_star_draws_five_known_segments() {
    let mut s = session();
    let mut out = Vec::new();
    let hs = added(&mut s, |s| out = run_example(s, "draw_star.rhai"));
    assert_eq!(out, ["Drew a star with 5 points at (0.0, 0.0)."]);
    assert_eq!(hs.len(), 5);
    // The example's own value of pi (not f64::consts::PI): the endpoints must match exactly.
    #[allow(clippy::approx_constant)]
    let pi = 3.14159265358979_f64;
    let p = |i: usize| {
        let a = pi / 2.0 + (i * 2) as f64 * 2.0 * pi / 5.0;
        [10.0 * a.cos(), 10.0 * a.sin()]
    };
    for (i, h) in hs.iter().enumerate() {
        let (a, b, _) = line_ends(&s, &h.hex());
        let (wa, wb) = (p(i), p((i + 1) % 5));
        assert!(near(a[0], wa[0]) && near(a[1], wa[1]) && near(b[0], wb[0]) && near(b[1], wb[1]), "segment {i}: {a:?}-{b:?}, want {wa:?}-{wb:?}");
    }
}

#[test]
fn example_draw_grid_draws_twelve_lines() {
    let mut s = session();
    let mut out = Vec::new();
    let hs = added(&mut s, |s| out = run_example(s, "draw_grid.rhai"));
    assert_eq!(out, ["Drew a 5x5 grid with 10.0 unit spacing."]);
    let mut got: Vec<([f64; 2], [f64; 2])> = hs
        .iter()
        .map(|h| {
            let (a, b, _) = line_ends(&s, &h.hex());
            (a, b)
        })
        .collect();
    let mut want: Vec<([f64; 2], [f64; 2])> = (0..=5)
        .map(|r| ([0.0, r as f64 * 10.0], [50.0, r as f64 * 10.0]))
        .chain((0..=5).map(|c| ([c as f64 * 10.0, 0.0], [c as f64 * 10.0, 50.0])))
        .collect();
    got.sort_by(|x, y| x.partial_cmp(y).unwrap());
    want.sort_by(|x, y| x.partial_cmp(y).unwrap());
    assert_eq!(got, want);
}

#[test]
fn example_list_layers_lists_each_layer() {
    let mut s = session();
    s.execute("layer.new", &json!({ "name": "Doors" })).unwrap();
    s.execute("layer.set", &json!({ "name": "Doors", "on": false })).unwrap();
    assert_eq!(run_example(&mut s, "list_layers.rhai"), ["Layers in the current drawing:", "  0 (on)", "  Defpoints (on)", "  Doors (off)"]);
}

#[test]
fn example_selection_info_names_the_selected_types() {
    let mut s = session();
    assert_eq!(run_example(&mut s, "selection_info.rhai"), ["Nothing selected."]);
    let l = s.execute("line", &json!({ "points": [[0, 0], [1, 0]] })).unwrap()["handles"][0].as_str().unwrap().to_string();
    let c = s.execute("circle", &json!({ "center": [5, 5], "radius": 1 })).unwrap()["handle"].as_str().unwrap().to_string();
    s.set_selection(vec![Handle::parse_hex(&l).unwrap(), Handle::parse_hex(&c).unwrap()]);
    assert_eq!(run_example(&mut s, "selection_info.rhai"), ["2 object(s) selected:".to_string(), format!("  [{l}] Line"), format!("  [{c}] Circle")]);
}

// ---- MUST 7: SECURELOAD ----

#[test]
fn rhairun_of_an_untrusted_file_is_refused_under_secureload_1() {
    let mut s = session();
    let dir = std::env::temp_dir().join(format!("cadaddy-rhai-untrusted-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("draw.rhai");
    std::fs::write(&path, r#"command("line", #{ points: [[0.0, 0.0], [1.0, 1.0]] }); 7"#).unwrap();
    assert_eq!(eval(&mut s, r#"getvar("SECURELOAD")"#), json!(1));
    let e = s.execute("rhairun", &json!({ "path": path.to_string_lossy() })).unwrap_err().to_string();
    assert!(e.contains("isn't in a trusted folder, so it wasn't loaded (SECURELOAD 1)"), "{e}");
    assert_eq!(s.doc().unwrap().model.iter().count(), 0, "nothing of it ran");
    // From a script: refused the same way, as a script error.
    let r = s.execute("rhai", &json!({ "code": format!("command(\"rhairun\", #{{ path: {:?} }})", path.to_string_lossy()) })).unwrap();
    assert!(r.get("error").is_some(), "{r}");
    // Trusted, it runs.
    s.settings.trustedpaths = dir.to_string_lossy().into_owned();
    assert_eq!(s.execute("rhairun", &json!({ "path": path.to_string_lossy() })).unwrap()["value"], json!(7));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_script_cannot_loosen_secureload() {
    let mut s = session();
    for code in [
        r#"setvar("SECURELOAD", 0)"#,
        r#"setvar("TRUSTEDPATHS", "/")"#,
        r#"command("secureload.allow")"#,
        r#"command("secureload.allow", #{ path: "x.rhai" })"#,
    ] {
        let r = s.execute("rhai", &json!({ "code": code })).unwrap();
        assert!(r.get("error").is_some(), "{code}: {r}");
    }
    assert_eq!(s.settings.secureload, 1);
    assert_eq!(s.settings.trustedpaths, Session::new().settings.trustedpaths);
    // A worker script can't either.
    s.rhai_start(r#"setvar("SECURELOAD", 0)"#).unwrap();
    let (e, _) = pump_until_done(&mut s, Duration::from_secs(5)).unwrap_err();
    assert!(e.contains("SECURELOAD"), "{e}");
    assert_eq!(s.settings.secureload, 1);
}
