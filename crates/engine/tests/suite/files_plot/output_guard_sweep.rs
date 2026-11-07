//! The path probe, in the repo: every command whose parameter docs name a file path, run by an
//! agent with no user to ask (`agent.confirm` off) onto (a) the open drawing's file, (b) another
//! existing drawing, (c) an existing PDF. None of them may change that file without `force: true`.

use cadaddy_engine::cmd::file::{IoHooks, set_io};
use cadaddy_engine::{Session, command_specs};
use serde_json::json;
use std::path::{Path, PathBuf};

const KEYS: [&str; 10] = ["path", "out", "output", "file", "dest", "to", "target", "filename", "outPath", "out_path"];

/// Commands the probe leaves alone: they end the session, run programs, or need a UI or a service.
const SKIP: [&str; 11] = ["job.start", "quit", "exit", "close", "closeall", "delay", "shell", "sh", "appload", "bim.from_pdf", "printset.links"];

fn hooks() {
    set_io(IoHooks {
        read: |b, name| cadaddy_io::read(b, name).map_err(|e| e.to_string()),
        write: |d, name| cadaddy_io::write(d, name).map_err(|e| e.to_string()),
        plot: Some(|d, space, opts| cadaddy_io::plotter::plot(d, space, opts).map_err(|e| e.to_string())),
        read_pdf: None,
        read_dwf: None,
        write_dwfx: None,
    });
}

/// Whether the params doc of a command lists `key` as a parameter (`{path?: …, file: …}`).
fn takes(doc: &str, key: &str) -> bool {
    doc.match_indices(key).any(|(i, _)| {
        let before = doc[..i].trim_end().chars().last();
        let after = doc[i + key.len()..].trim_start().trim_start_matches('?').trim_start();
        matches!(before, Some('{' | ',')) && after.starts_with(':')
    })
}

fn dir() -> PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-outsweep-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn text(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

#[test]
fn no_command_changes_an_existing_file_without_force() {
    hooks();
    let d = dir();
    let mut changed = Vec::new();
    let mut probed = 0;
    for spec in command_specs() {
        if SKIP.contains(&spec.id) || spec.id.starts_with("agent.") {
            continue;
        }
        for key in KEYS.into_iter().filter(|k| takes(spec.params, k)) {
            for mode in ["open", "other", "otherpdf"] {
                let name = format!("{}_{key}_{mode}", spec.id.replace(['.', '-'], "_"));
                let mut s = Session::new();
                s.execute("line", &json!({"points": [[0, 0], [100, 50]]})).unwrap();
                let own = d.join(format!("own_{name}.dxf"));
                s.execute("saveas", &json!({ "path": text(&own) })).unwrap();
                s.agent.confirm = false;
                let target = match mode {
                    "open" => own.clone(),
                    "other" => d.join(format!("other_{name}.dxf")),
                    _ => d.join(format!("other_{name}.pdf")),
                };
                if mode != "open" {
                    std::fs::write(&target, format!("precious {name}")).unwrap();
                }
                let before = std::fs::read(&target).unwrap();
                probed += 1;
                let _ = s.execute_agent(spec.id, &json!({ key: text(&target) }));
                s.cancel();
                if std::fs::read(&target).ok().as_deref() != Some(before.as_slice()) {
                    changed.push(format!("{} {key} {mode}", spec.id));
                }
            }
        }
    }
    let _ = std::fs::remove_dir_all(&d);
    assert!(probed > 100, "probed {probed} command/key/mode combinations");
    assert!(changed.is_empty(), "{} of {probed} changed a file without force:\n{}", changed.len(), changed.join("\n"));
}
