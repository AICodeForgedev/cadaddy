//! Engine-level tests for the print-set project: two-session isolation, full-hotel through
//! the command path, and verification of the unified pipeline (MUST 1 and MUST 6 from readerwire).

use cadaddy_engine::Session;
use cadaddy_mepfix::generate_hotel;
use cadaddy_mepfix::hotel::HotelSize;
use serde_json::json;

// ─── Verify MUST 1: printset.run command exists and is reachable ─────────────

#[test]
fn printset_run_command_is_registered() {
    assert!(cadaddy_engine::cmd::find_by_id_or_alias("printset.run").is_some(), "printset.run command should be registered");
    assert!(cadaddy_engine::cmd::find_by_id_or_alias("printset.open").is_some(), "printset.open command should be registered");
    assert!(cadaddy_engine::cmd::find_by_id_or_alias("printset.classify").is_some(), "printset.classify command should be registered");
    assert!(cadaddy_engine::cmd::find_by_id_or_alias("printset.dashboard").is_some(), "printset.dashboard command should be registered");
}

#[test]
fn printset_run_without_project_gives_error() {
    let mut s = Session::new();
    let result = s.execute("printset.run", &json!({}));
    assert!(result.is_err(), "printset.run without a project should error");
    let err = format!("{}", result.unwrap_err());
    assert!(err.contains("printset.open"), "error should mention printset.open: {err}");
}

// ─── Verify MUST 6: two sessions, independent projects ──────────────────────

#[test]
fn two_sessions_have_independent_projects() {
    let set_a = generate_hotel(42, HotelSize::Small).expect("generate hotel A");
    let set_b = generate_hotel(99, HotelSize::Small).expect("generate hotel B");

    let dir_a = std::env::temp_dir().join(format!("cadaddy_ps_sess_a_{}", std::process::id()));
    let dir_b = std::env::temp_dir().join(format!("cadaddy_ps_sess_b_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir_a);
    let _ = std::fs::remove_dir_all(&dir_b);
    set_a.write_to(&dir_a).expect("write A");
    set_b.write_to(&dir_b).expect("write B");

    let mut s1 = Session::new();
    let mut s2 = Session::new();

    // Open different folders in each session.
    let r1 = s1.execute("printset.open", &json!({"path": dir_a.display().to_string(), "name": "Hotel A"})).expect("open A");
    let r2 = s2.execute("printset.open", &json!({"path": dir_b.display().to_string(), "name": "Hotel B"})).expect("open B");

    assert_eq!(r1["name"], "Hotel A");
    assert_eq!(r2["name"], "Hotel B");

    // Each session's dashboard should reflect its own project.
    let d1 = s1.execute("printset.dashboard", &json!({})).expect("dashboard A");
    let d2 = s2.execute("printset.dashboard", &json!({})).expect("dashboard B");

    assert_eq!(d1["name"], "Hotel A");
    assert_eq!(d2["name"], "Hotel B");
    // They should both have sheets and not share state.
    assert!(d1["sheets"].as_u64().unwrap_or(0) > 0, "session 1 should have sheets");
    assert!(d2["sheets"].as_u64().unwrap_or(0) > 0, "session 2 should have sheets");

    let _ = std::fs::remove_dir_all(&dir_a);
    let _ = std::fs::remove_dir_all(&dir_b);
}

// ─── Full hotel through command path (MUST 7) ───────────────────────────────

#[test]
fn full_hotel_through_command_path() {
    let set = generate_hotel(100, HotelSize::Full).expect("generate full hotel");

    let dir = std::env::temp_dir().join(format!("cadaddy_ps_cmd_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    set.write_to(&dir).expect("write fixture set");

    let mut s = Session::new();

    // printset.open
    let open_result = s.execute("printset.open", &json!({"path": dir.display().to_string(), "name": "Full Hotel"})).expect("open");
    assert_eq!(open_result["name"], "Full Hotel");
    let sheet_count = open_result["sheets"].as_u64().unwrap_or(0);
    assert!(sheet_count > 20, "full hotel should have >20 sheets, got {sheet_count}");

    // printset.classify
    let classify_result = s.execute("printset.classify", &json!({})).expect("classify");
    assert_eq!(classify_result["classified"].as_u64().unwrap_or(0), sheet_count, "all sheets should be classified");

    // printset.run (runs all stages; readers are stubs so items=0, but the command should work)
    let run_result = s.execute("printset.run", &json!({})).expect("run");
    assert!(run_result["stages_run"].as_u64().unwrap_or(0) > 0, "should have run stages");

    // printset.dashboard
    let dash = s.execute("printset.dashboard", &json!({})).expect("dashboard");
    assert_eq!(dash["name"], "Full Hotel");
    assert!(dash["sheets"].as_u64().unwrap_or(0) > 0);

    // printset.sheets — verify the listing command works (no discipline filter since
    // the engine classify runs without a DXF loader, so file-only projects don't get disciplines).
    let all_sheets = s.execute("printset.sheets", &json!({})).expect("all sheets");
    assert!(all_sheets["total"].as_u64().unwrap_or(0) > 0, "should list all sheets");

    let _ = std::fs::remove_dir_all(&dir);
}
