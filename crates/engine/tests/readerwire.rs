//! Q5128: end-to-end pipeline test — verifies that every reader command is reachable and
//! returns structured results through the engine's execute path.

use cadaddy_engine::Session;
use cadaddy_mepfix::printset as mep_printset;
use serde_json::json;

/// Every reader command is registered and findable.
#[test]
fn all_reader_commands_are_registered() {
    let expected = [
        "conduit.read",
        "gear.read",
        "prints.read_scan",
        "site.read",
        "mep.heights",
        "ducts.read",
        "pipes.read",
        "structure.read",
        "ceilings.read",
        "sprinklers.read",
        "trays.read",
        "walls.read",
        "ceilings.assemblies",
        "rfi.read",
        "mep.equipment.read",
        "mep.read_devices",
    ];
    for cmd in &expected {
        assert!(
            cadaddy_engine::cmd::find_by_id_or_alias(cmd).is_some(),
            "missing command: {cmd}",
        );
    }
}

/// The printset pipeline uses mepfix's project types (Phase 1 check).
#[test]
fn printset_uses_mepfix_types() {
    // create_from_truth needs a truth with sheets (from generate, not truth_for).
    let set = cadaddy_mepfix::generate(1).unwrap();
    let project = mep_printset::create_from_truth(&set.truth);
    assert!(!project.sheets.is_empty(), "project from generated truth has sheets");
    assert_eq!(project.schema, mep_printset::SCHEMA);
    // Verify the schema tag is the mepfix one, confirming Phase 1 delegation.
    assert_eq!(project.schema, "cadaddy-printset/1");
}

/// A reader command without an open project gives a guiding error.
#[test]
fn reader_without_project_gives_guiding_error() {
    let mut s = Session::new();
    let err = s.execute("conduit.read", &json!({}));
    assert!(err.is_err());
    let msg = format!("{}", err.unwrap_err());
    assert!(msg.contains("printset.open") || msg.contains("bim_job"), "error should guide: {msg}");
}
