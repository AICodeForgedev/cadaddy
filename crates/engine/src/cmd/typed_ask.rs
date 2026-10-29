//! Commands that take their arguments as JSON parameters and have no prompts of their own, typed
//! at the command line with nothing after the name (`DELAY`, `UCSICON`, `EXPORT`…): they ask for
//! what is missing the way AutoCAD does ("Enter delay time (in milliseconds):") instead of
//! answering "invalid parameters for `delay`: `ms` …". The questions are [`super::ask`] steps; each
//! answer becomes one JSON parameter and the command then runs as if called with them.

use super::ask::{Ask, Kind, ask, ask_or, machine_of};
use crate::Interactive;

/// The questions of each command, in the order they are asked.
fn questions(id: &str) -> Option<Vec<Ask>> {
    let file = |key, message| ask(key, message, Kind::Text);
    let word = |key, message, words| ask(key, message, Kind::Keyword(words));
    Some(match id {
        "ucsicon" => vec![ask_or("option", "Enter an option", Kind::Keyword(&["ON", "OFF", "Origin", "Noorigin"]), |_| Some("ON".into()))],
        "delay" => vec![ask("ms", "Enter delay time (in milliseconds)", Kind::Number)],
        "vpoint" => vec![ask("dir", "Specify a view point (x,y,z)", Kind::Point3)],
        "saveimg" => vec![file("path", "Enter file name for the image (.png or .jpg)")],
        "dwgconvert" | "convert" => vec![
            file("folder", "Enter the folder of drawings to convert"),
            ask_or("to", "Enter output format", Kind::Keyword(&["CDY", "DWG", "DXF"]), |_| Some("DWG".into())),
        ],
        "import" => vec![file("path", "Enter file name to import")],
        "export" => vec![file("path", "Enter file name to export (its extension picks the format)")],
        "lisp" => vec![file("code", "Enter a LISP expression")],
        "scriptcall" => vec![file("path", "Enter script file name")],
        "adcenter" => vec![file("path", "Enter drawing file name")],
        "dimstyle" => vec![file("name", "Enter dimension style name")],
        "style" => vec![file("name", "Enter text style name")],
        "pointcloudattach" => vec![file("path", "Enter point cloud file name (.las, .pts or .xyz)")],
        "markupimport" => vec![file("path", "Enter PDF file name")],
        "markupstatus" => {
            vec![file("id", "Enter markup id"), ask_or("status", "Enter status", Kind::Keyword(&["Open", "Done"]), |_| Some("done".into()))]
        }
        "bremove" => vec![file("block", "Enter block name"), file("element", "Enter parameter label, action name or graph id to remove")],
        "bparameter" => vec![
            file("block", "Enter block name"),
            word(
                "type",
                "Enter parameter type",
                &["Point", "Linear", "Polar", "Xy", "Rotation", "Alignment", "Flip", "Visibility", "Lookup", "Basepoint"],
            ),
        ],
        "bactiontool" => vec![
            file("block", "Enter block name"),
            word("type", "Enter action type", &["Move", "Stretch", "Scale", "Rotate", "Flip", "Array", "Polarstretch", "Lookup"]),
            file("parameter", "Enter the parameter's label or graph id"),
        ],
        "layer.new" => vec![file("name", "Enter name for new layer")],
        "layer.set" => vec![file("name", "Enter name of the layer to make current")],
        "bvstate" => vec![file("block", "Enter block name")],
        "sheetset" => vec![file("set", "Enter sheet set file name (.sheetset.json)")],
        "publishlist" => vec![word("action", "Enter an option", &["Save", "Load", "Layouts"]), file("path", "Enter sheet list file name")],
        _ => return None,
    })
}

/// The prompts for `id` typed with nothing after it, when it has them.
pub(crate) fn machine_for(id: &str) -> Option<Box<dyn Interactive>> {
    let spec = super::find_command(id)?;
    Some(machine_of(spec.id, spec.id, questions(spec.id)?))
}

/// The commands that ask for their parameters when typed bare (for the tests).
pub fn asking_commands() -> Vec<&'static str> {
    super::command_specs().iter().map(|c| c.id).filter(|id| questions(id).is_some()).collect()
}
