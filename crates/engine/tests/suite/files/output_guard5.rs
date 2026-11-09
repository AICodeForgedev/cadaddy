//! A hard link to the open drawing under a data name (`plan.cdy` linked as `plan.csv`) is the open
//! drawing too: the path text differs, the file is the same one. The output guard compares files
//! by identity (device and file number), so TAKEOFF, BIM.TAKEOFF, ESTIMATE.EXPORT, BIM.BOM and
//! every other writer refuse it, `force: true` or not. A sweep runs every command whose documented
//! parameters name an output file against hard links to the open drawing, so a writer that skips
//! the guard (or a guard that goes back to comparing names) fails here.

use crate::helper::{scratch, serial};
use crate::output_guard4::{DATA_EXTS, NOT_OUTPUT_WRITERS, OUTPUT_KEYS, built};
use cadaddy_engine::{Session, command_specs, documented_params, params_doc};
use serde_json::{Value, json};
use std::path::Path;

fn text(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// `link` as a fresh hard link to `target` (a writer that replaced the old link through a temporary
/// file left a separate file behind).
fn relink(target: &str, link: &Path) {
    let _ = std::fs::remove_file(link);
    std::fs::hard_link(target, link).unwrap_or_else(|e| panic!("hard link {}: {e}", link.display()));
}

/// The writers the test found writing through the link, each with the parameter naming its output.
fn writers() -> Vec<(&'static str, &'static str, Value)> {
    vec![("takeoff", "file", json!({})), ("bim.takeoff", "file", json!({})), ("estimate.export", "path", json!({})), ("bim.bom", "file", json!({}))]
}

#[test]
fn a_hard_link_to_the_open_drawing_is_refused_by_the_data_writers() {
    let _turn = serial();
    let d = scratch("outguard5-link");
    for name in ["hl_open.dxf", "hl_open.cdy"] {
        let (mut s, path) = built(&d, name);
        let before = std::fs::read(&path).unwrap();
        let link = d.join(format!("{name}-hard.csv"));
        for (cmd, key, p) in writers() {
            for force in [false, true] {
                relink(&path, &link);
                let mut q = p.clone();
                q[key] = json!(text(&link));
                q["force"] = json!(force);
                let r = s.execute(cmd, &q);
                let e = match r {
                    Ok(v) => panic!("{cmd} {key}=<hard link to {name}> (force {force}) ran: {v}"),
                    Err(e) => e.to_string(),
                };
                assert!(e.contains("open drawing"), "{cmd} through a link to {name}: {e}");
                assert_eq!(std::fs::read(&path).unwrap(), before, "{cmd} (force {force}) changed the open drawing {name} through a hard link");
            }
        }
    }
}

#[test]
fn a_separate_file_with_the_same_bytes_is_not_the_open_drawing() {
    let _turn = serial();
    let d = scratch("outguard5-copy");
    let (mut s, path) = built(&d, "plan.cdy");
    // A copy is another file: with force it is replaced, and the drawing stays as it was.
    let copy = d.join("plan-copy.csv");
    let before = std::fs::read(&path).unwrap();
    std::fs::write(&copy, &before).unwrap();
    s.execute("takeoff", &json!({ "file": text(&copy), "force": true })).unwrap_or_else(|e| panic!("takeoff over a copy: {e}"));
    assert_ne!(std::fs::read(&copy).unwrap(), before, "takeoff with force kept the copy's old bytes");
    assert_eq!(std::fs::read(&path).unwrap(), before, "takeoff over a copy changed the drawing");
}

#[test]
fn opening_a_hard_link_to_an_open_drawing_switches_to_it() {
    let _turn = serial();
    let d = scratch("outguard5-open");
    let (_, path) = built(&d, "plan.cdy");
    let link = d.join("other-name.cdy");
    relink(&path, &link);
    let mut s = Session::new();
    s.execute("open", &json!({ "path": path })).unwrap();
    let open = s.docs.len();
    let r = s.execute("open", &json!({ "path": text(&link) })).unwrap();
    assert_eq!(r["alreadyOpen"], json!(true), "a second name of the open file opened a second copy: {r}");
    assert_eq!(s.docs.len(), open, "two drawings for one file");
}

#[test]
fn no_command_writes_over_the_open_drawing_through_a_hard_link() {
    let _turn = serial();
    let d = scratch("outguard5-sweep");
    let (_, path) = built(&d, "victim.cdy");
    let before = std::fs::read(&path).unwrap();
    let mut changed = Vec::new();
    let mut tried = 0;
    for spec in command_specs() {
        if NOT_OUTPUT_WRITERS.contains(&spec.id) {
            continue;
        }
        let names = documented_params(params_doc(spec));
        let keys: Vec<&str> = OUTPUT_KEYS.iter().copied().filter(|k| names.contains(k)).collect();
        if keys.is_empty() {
            continue;
        }
        let mut s = Session::new();
        s.execute("open", &json!({ "path": path })).unwrap();
        for key in keys {
            for ext in DATA_EXTS {
                tried += 1;
                let link = d.join(format!("alias.{ext}"));
                relink(&path, &link);
                let _ = s.execute(spec.id, &json!({ key: text(&link), "force": true }));
                if std::fs::read(&path).map_or(true, |now| now != before) {
                    changed.push(format!("{} {{{key}: <a .{ext} hard link to the open drawing>}}", spec.id));
                    std::fs::write(&path, &before).unwrap();
                }
            }
        }
    }
    eprintln!("hard-link sweep: {tried} probes, {} wrote over the open drawing", changed.len());
    assert!(tried > 500, "probed {tried} output parameters");
    assert!(changed.is_empty(), "wrote over the open drawing through a hard link:\n{}", changed.join("\n"));
}
