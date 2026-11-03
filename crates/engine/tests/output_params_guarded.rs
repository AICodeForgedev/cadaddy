//! A command that writes a file from one of its parameters goes through `exchange::guard_output` /
//! `guard_params` (not a drawing file name, never an open drawing, an existing file only with
//! `force: true`). This scan fails when a file under `src/cmd` that writes with `writefile::write*` /
//! `write_file(` is neither guarded nor on the list of writers that predate the helper: a new writer
//! calls the helper (or is added below with the reason it needs none).

use std::path::{Path, PathBuf};

/// Files whose output commands call the helper.
const GUARDED: &[&str] = &[
    "cmd/bimwork/takeoff.rs",
    "cmd/blocks/extract.rs",
    "cmd/bom.rs",
    "cmd/clashreport.rs",
    "cmd/classic/attext.rs",
    "cmd/estimate.rs",
    "cmd/exchange.rs",
    "cmd/fieldview.rs",
    "cmd/ifcmodel.rs",
    "cmd/projrules.rs",
    "cmd/render.rs",
    "cmd/rfi.rs",
    "cmd/table/cells.rs",
    "cmd/takeoff.rs",
    "cmd/underlay.rs",
    "cmd/validate.rs",
    "cmd/verify.rs",
];

/// Writers outside the output commands of the guard task: autosaves and recovery files, other
/// commands' side files, writes into a folder the command made itself. Their paths either come from
/// the session, or are an agent's `path` kept from replacing an existing file by the overwrite rail
/// in `agent.rs`. Moving one to the helper is welcome; adding one here needs a reason in review.
const UNCHECKED: &[&str] = &[
    "cmd/actions.rs",
    "cmd/asbuilt.rs",
    "cmd/audit.rs",
    "cmd/bcf.rs",
    "cmd/blocks.rs",
    "cmd/calibrate.rs",
    "cmd/cam.rs",
    "cmd/classic/filter.rs",
    "cmd/classic/mkltype.rs",
    "cmd/classic/shapes.rs",
    "cmd/clipboard.rs",
    "cmd/cobie.rs",
    "cmd/concrete.rs",
    "cmd/coordination.rs",
    "cmd/copricing.rs",
    "cmd/dgn.rs",
    "cmd/drywall.rs",
    "cmd/estimate/outputs.rs",
    "cmd/express/attio.rs",
    "cmd/file.rs",
    "cmd/file/compare.rs",
    "cmd/file/convert.rs",
    "cmd/file/etransmit.rs",
    "cmd/file/sheetset.rs",
    "cmd/file/zip.rs",
    // bim.form.promote: only its own trade template file in the settings folder, updated in place
    "cmd/forms.rs",
    "cmd/fourd.rs",
    "cmd/guided.rs",
    "cmd/hatch/patfile.rs",
    "cmd/labels.rs",
    "cmd/layer/states.rs",
    "cmd/layout.rs",
    "cmd/layout/plotstyle.rs",
    "cmd/layout/template.rs",
    "cmd/lightbake.rs",
    "cmd/mepqty.rs",
    "cmd/p2meval.rs",
    "cmd/perceive/mod.rs",
    "cmd/plotters.rs",
    "cmd/printset.rs",
    "cmd/procure.rs",
    "cmd/progress.rs",
    "cmd/project.rs",
    // writes only inside the project folders it manages (registry, shelved archive, template copy)
    "cmd/project_hub.rs",
    // writes only its own price database (quotes, map, history, settings), updated in place
    "cmd/pricebook.rs",
    "cmd/props/linfile.rs",
    "cmd/propuse.rs",
    "cmd/rasterout.rs",
    "cmd/sequence.rs",
    "cmd/wholejob.rs",
];

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(read) = std::fs::read_dir(dir) else { return };
    for e in read.flatten() {
        let p = e.path();
        if p.is_dir() {
            sources(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

fn writes_a_file(text: &str) -> bool {
    let prod = text.split("#[cfg(test)]").next().unwrap_or(text);
    ["writefile::write(", "writefile::write_atomic(", "writefile::create(", "write_file("].iter().any(|n| prod.contains(n))
}

#[test]
fn every_output_writer_is_guarded_or_listed() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    sources(&src.join("cmd"), &mut files);
    assert!(files.len() > 50, "found the sources");
    let mut bad = Vec::new();
    for f in files {
        let rel = format!("cmd/{}", f.strip_prefix(src.join("cmd")).unwrap().to_string_lossy().replace('\\', "/"));
        let text = std::fs::read_to_string(&f).unwrap();
        let guarded = text.contains("guard_output(") || text.contains("guard_params(") || text.contains("guard_image_output(");
        if GUARDED.contains(&rel.as_str()) {
            if !guarded {
                bad.push(format!("{rel} is listed as guarded but doesn't call guard_output / guard_params"));
            }
        } else if writes_a_file(&text) && !guarded && !UNCHECKED.contains(&rel.as_str()) {
            bad.push(format!("{rel} writes a file: route its output path through exchange::guard_output"));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

/// The tools outside the engine (the MCP server, the GUI's control bridge) that write a file from
/// an agent's `path` argument have no command to carry the guard: each calls the engine's
/// `guard_image_output` before it writes, and a new writer there fails this test until it does.
#[test]
fn mcp_and_control_writers_are_guarded() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    sources(&root.join("mcp/src"), &mut files);
    sources(&root.join("ui-egui/src"), &mut files);
    files.retain(|f| {
        let name = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let in_mcp = f.components().any(|c| c.as_os_str() == "mcp");
        (in_mcp && name != "tests.rs") || name == "control.rs"
    });
    assert!(files.len() >= 3, "found the sources");
    let mut bad = Vec::new();
    for f in files {
        let text = std::fs::read_to_string(&f).unwrap();
        let prod = text.split("#[cfg(test)]").next().unwrap_or(&text);
        let writes = ["fs::write(", "File::create(", "writefile::write", "write_file("].iter().any(|n| prod.contains(n));
        if writes && !prod.contains("guard_image_output(") && !prod.contains("guard_output(") {
            bad.push(format!("{} writes a file from an agent's argument: call guard_image_output first", f.display()));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}
