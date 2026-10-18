//! Every menu command either prompts when started or runs without parameters: none answers
//! "`x` is required" (battle test 3: Find Text and Write Block on the ribbon did). The ones listed
//! open a dialog in the app, or report a state ("No isolated layers.").

use cadaddy_engine::{Session, command_specs};

const DIALOG_OR_STATE: &[&str] = &[
    "open",
    "qsave",
    "saveas",
    "layuniso",
    "layerp",
    "layerstate.save",
    "plot",
    "exportpdf",
    "mspace",
    "pspace",
    "pdfimport",
    "import",
    "export",
    "publish",
    "pdfbook",
    "style",
    "dimstyle",
    "dimstyle.dimension",
];

#[test]
fn menu_commands_never_demand_parameters() {
    let mut bad = Vec::new();
    for c in command_specs() {
        if (c.menu.is_empty() && c.aliases.is_empty()) || DIALOG_OR_STATE.contains(&c.id) {
            continue;
        }
        let mut s = Session::new();
        s.execute("circle", &serde_json::json!({"center": [0, 0], "radius": 1})).unwrap();
        if let Err(e) = s.start(c.id) {
            let m = e.to_string();
            if m.contains("required") || m.contains("invalid parameters") {
                bad.push(format!("{}: {m}", c.id));
            }
        }
        s.cancel();
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}
