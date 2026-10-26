//! `cadaddy-cli` explains itself: help, help COMMAND, guide, COMMAND --help (Q5279).

use std::process::{Command, Output};

fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cadaddy-cli")).args(args).output().unwrap()
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

#[test]
fn help_lists_the_main_workflow_with_commands() {
    for args in [&["help"][..], &["--help"], &["-h"], &[]] {
        let o = cli(args);
        assert!(o.status.success(), "{args:?}");
        let text = stdout(&o);
        for want in ["cadaddy-cli bim prints --out model.cdy", "bim.takeoff", "bim.estimate", "cadaddy-cli guide", "cadaddy-cli help COMMAND"] {
            assert!(text.contains(want), "{args:?}: no `{want}` in\n{text}");
        }
    }
}

#[test]
fn help_command_prints_summary_parameters_and_example() {
    let o = cli(&["help", "bim.takeoff"]);
    assert!(o.status.success());
    let text = stdout(&o);
    assert!(text.starts_with("BIM.TAKEOFF — Model Takeoff"), "{text}");
    assert!(
        text.contains("group_by") && text.contains("Parameters (JSON):") && text.contains("cadaddy-cli run DRAWING.dxf --cmd 'bim.takeoff {}'"),
        "{text}"
    );
    // A subcommand, and the same through --help.
    let bim = stdout(&cli(&["help", "bim"]));
    assert!(bim.contains("cadaddy-cli bim FILE.pdf|FOLDER") && bim.contains("Example:"), "{bim}");
    assert_eq!(stdout(&cli(&["bim", "--help"])), bim);
    // Unknown names fail with a pointer.
    let o = cli(&["help", "no-such-command-here"]);
    assert!(!o.status.success() && String::from_utf8_lossy(&o.stderr).contains("cadaddy-cli help"));
}

#[test]
fn guide_is_the_mcp_workflows_text() {
    let o = cli(&["guide"]);
    assert!(o.status.success());
    let mut b = cadaddy_mcp::Headless::default();
    let r = cadaddy_mcp::tools::call_tool(&mut b, "workflows", &serde_json::json!({})).unwrap();
    let v: serde_json::Value = serde_json::from_str(r["content"][0]["text"].as_str().unwrap()).unwrap();
    let guide = v["guide"].as_str().unwrap();
    assert_eq!(stdout(&o), format!("{guide}\n"));
    assert!(guide.contains("pdf-to-bim") && guide.contains("cli: cadaddy-cli bim prints"));
    // The structured flows carry the same shell commands.
    let flow = v["workflows"].as_array().unwrap().iter().find(|f| f["name"] == "pdf-to-bim").unwrap();
    assert_eq!(flow["cli"][0], "cadaddy-cli bim prints --out model.cdy");
}

#[test]
fn unknown_command_points_at_help() {
    let o = cli(&["status"]);
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("unknown command `status`"));
}
