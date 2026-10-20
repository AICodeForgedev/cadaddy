//! End to end: workspaces. WSCURRENT switched to the Electrical example hides ribbon tabs (Parametric,
//! Manage) and opens on Insert; the arrangement saved as a workspace of the user's comes back after
//! switching away; Drafting & Annotation brings every tab back. Checked on the controls on screen.

mod e2e_common;

use e2e_common::E2e;
use serde_json::json;

fn current(e: &mut E2e) -> String {
    e.exec("workspace", json!({}))["current"].as_str().unwrap_or_default().to_string()
}

#[test]
fn switching_and_saving_workspaces_changes_the_interface() {
    let mut e = E2e::new();
    let drafting = current(&mut e);
    assert!(e.shows("Parametric") && e.shows("Manage"), "every ribbon tab in {drafting}");

    e.exec("setvar", json!({ "name": "WSCURRENT", "value": "Electrical" }));
    e.pump();
    assert_eq!(current(&mut e), "Electrical");
    assert!(!e.shows("Parametric") && !e.shows("Manage"), "Electrical hides Parametric and Manage");
    assert!(e.shows("Insert") && e.shows("Output"));

    // Saved as the user's own, then back after another switch.
    let r = e.exec("workspace", json!({ "action": "save", "name": "Project A" }));
    assert!(r["workspaces"].as_array().unwrap().iter().any(|w| w["name"] == "Project A" && w["builtIn"] == false), "{r}");
    e.exec("workspace", json!({ "name": drafting }));
    assert!(e.shows("Parametric"), "Drafting & Annotation brings the tabs back");
    e.exec("workspace", json!({ "name": "Project A" }));
    assert_eq!(current(&mut e), "Project A");
    assert!(!e.shows("Parametric") && e.shows("Insert"), "the saved arrangement is restored");

    // From the menu too.
    e.menu("ui.workspace.modeling");
    assert_eq!(current(&mut e), "3D Modeling");
    e.menu("ui.workspace.save");
    assert_eq!(e.dialog().as_deref(), Some("workspace.save"));
    e.exec("workspace", json!({ "action": "delete", "name": "Project A" }));
    assert!(!e.exec("workspace", json!({}))["workspaces"].to_string().contains("Project A"));
}
