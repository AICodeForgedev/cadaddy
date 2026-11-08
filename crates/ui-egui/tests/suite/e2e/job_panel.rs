//! Q6266 (test-asus-winsmoke: a guided job could only be started by typing `job.start {json}`):
//! the guided job from the window, as a person runs it. The menus (Tools ▸ Guided Job ▸ Start
//! Job..., Window ▸ Job), the ribbon's Automation ▸ Job and `job.start` typed alone open the Job
//! panel; its start form takes the prints folder (from the folder dialog), the goal and the mode;
//! Start runs the job; the panel then shows where it is, its tabs, the question it waits on with
//! its answer buttons, and its report.

use crate::e2e_common::{E2e, folder};
use serde_json::{Value, json};

fn rect(v: &Value) -> [f64; 4] {
    let r: Vec<f64> = v["rect"].as_array().into_iter().flatten().filter_map(Value::as_f64).collect();
    [r.first().copied().unwrap_or(0.0), r.get(1).copied().unwrap_or(0.0), r.get(2).copied().unwrap_or(0.0), r.get(3).copied().unwrap_or(0.0)]
}

/// Click the button labelled `label` (the file tabs have a "Start" tab too).
fn click_button(e: &mut E2e, label: &str) -> bool {
    let Some(x) = e.elements().into_iter().find(|x| x["label"].as_str() == Some(label) && x["role"] == "button") else { return false };
    let [x, y, w, h] = rect(&x);
    e.ok("ui.click", json!({ "x": x + w / 2.0, "y": y + h / 2.0, "space": "screen" }));
    true
}

fn docked(e: &mut E2e, panel: &str) -> bool {
    e.ok("ui.inspect", json!({}))["ui"]["docking"].to_string().contains(&format!("\"{panel}\""))
}

fn labels(e: &mut E2e) -> Vec<String> {
    e.elements().iter().filter_map(|x| x["label"].as_str().map(str::to_string)).collect()
}

/// Every menu item's command, depth first.
fn menu_commands(tree: &Value, out: &mut Vec<(String, String)>, path: &str) {
    for e in tree.as_array().into_iter().flatten() {
        let label = e["label"].as_str().unwrap_or_default();
        let here = if path.is_empty() { label.to_string() } else { format!("{path} > {label}") };
        if let Some(c) = e["command"].as_str() {
            out.push((c.to_string(), here.clone()));
        }
        menu_commands(&e["children"], out, &here);
    }
}

#[test]
fn the_menus_the_ribbon_and_the_command_line_open_the_job_panel() {
    let mut e = E2e::new();
    // The menu tree has the job (it had no job item at all).
    let mut items = Vec::new();
    menu_commands(&e.ok("ui.menu.list", json!({})), &mut items, "");
    let job_items: Vec<&(String, String)> = items.iter().filter(|(c, _)| c.starts_with("job.") || c == "ui.panel.job").collect();
    assert!(job_items.iter().any(|(c, p)| c == "job.start" && p == "Tools > Guided Job > Start Job..."), "{job_items:?}");
    assert!(job_items.iter().any(|(c, p)| c == "ui.panel.job" && p == "Window > Job"), "{job_items:?}");
    // The engine reports job.start's menu path (it was [] for every job command).
    let cmds = e.ok("engine.commands", json!({ "filter": "job.start" }));
    let start = cmds.as_array().into_iter().flatten().find(|c| c["id"] == "job.start").cloned().expect("job.start");
    assert!(start.to_string().contains("Guided Job"), "{start}");

    // Automation ▸ Job.
    assert!(e.click("Automation"));
    assert!(click_button(&mut e, "Job"), "the ribbon's Job button: {:?}", labels(&mut e));
    assert!(docked(&mut e, "job"), "the Job panel is open");
    for l in ["Prints folder", "Goal", "Stop at each question", "Run unattended (take each default)", "Keep the job's drawing", "Browse..."] {
        assert!(e.shows(l), "{l} in the start form: {:?}", labels(&mut e));
    }
    assert!(e.elements().iter().any(|x| x["label"] == "Start" && x["role"] == "button"), "Start");

    // Closed, then each other way in opens it again.
    for how in ["menu job.start", "menu ui.panel.job", "typed"] {
        e.exec("ui.dock", json!({ "operation": "close", "panel": "job" }));
        assert!(!e.shows("Prints folder"), "closed before {how}");
        match how {
            "typed" => {
                e.typed("job.start");
            }
            _ => {
                e.menu(how.trim_start_matches("menu "));
            }
        }
        assert!(docked(&mut e, "job"), "{how} opens the Job panel");
        assert!(e.shows("Prints folder"), "{how}: the start form");
    }
    // No job was started by any of them.
    assert!(e.exec("job.status", json!({}))["job_id"].is_null());
}

#[test]
fn a_job_runs_from_the_job_panel_and_shows_its_question_and_report() {
    let mut e = E2e::new();
    let prints = folder("job-panel-prints");
    let path = prints.to_string_lossy().replace('\\', "/");
    let picked = path.clone();
    e.app.services.pick_folder = Some(Box::new(move || Some(picked.clone())));
    e.menu("ui.panel.job");

    // Start without a folder: the form says what is missing; no job starts.
    assert!(click_button(&mut e, "Start"));
    assert!(e.shows("Choose the prints folder first."), "{:?}", labels(&mut e));
    assert!(e.exec("job.status", json!({}))["job_id"].is_null());

    // The folder from the folder dialog, then Start: the job runs (off the UI thread; the
    // harness's frames wait for it). The folder is still empty: the job stops at its import and
    // the panel says why.
    assert!(click_button(&mut e, "Browse..."));
    assert!(click_button(&mut e, "Start"));
    let st = e.exec("job.status", json!({}));
    assert_eq!(st["status"], "error", "{st}");
    let shown = labels(&mut e);
    assert!(shown.iter().any(|l| l.starts_with("Stopped at import: no drawing files found")), "{shown:?}");
    assert!(shown.iter().any(|l| l == "New job"), "{shown:?}");
    assert!(!shown.iter().any(|l| l == "Continue"), "a stopped job can't go on: {shown:?}");
    let stopped = e.app.session.state().unwrap().uid;

    // A sheet in the folder, then New job: the form again, with the folder kept; Start opens the
    // new job in a new drawing (the stopped one stays in its own).
    let mut sheet = cadaddy_engine::Session::new();
    sheet.execute("line", &json!({ "points": [[0, 0], [1200, 0]] })).unwrap();
    sheet.execute("saveas", &json!({ "path": prints.join("A-101.dxf") })).unwrap();
    assert!(click_button(&mut e, "New job"));
    assert!(e.shows("Prints folder") && e.shows("Back to the job"), "{:?}", labels(&mut e));
    assert!(click_button(&mut e, "Start"));
    assert_ne!(e.app.session.state().unwrap().uid, stopped, "a new drawing");
    let st = e.exec("job.status", json!({}));
    assert!(st["job_id"].is_string(), "a job: {st}");
    assert_eq!(st["goal"], "estimate", "the default goal: {st}");
    let report = e.exec("job.report", json!({}));
    assert_eq!(report["pdfs"].as_str().map(|p| p.replace('\\', "/")), Some(path.clone()), "{report}");
    let shown = labels(&mut e);
    assert!(shown.iter().any(|l| l == "Job: Estimate"), "{shown:?}");
    for l in ["Worklist", "Progress", "Findings", "Rules", "Report"] {
        assert!(shown.iter().any(|x| x == l), "the {l} tab: {shown:?}");
    }

    // The question it waits on, with its answer buttons; a click answers it.
    let open = st["open_questions"].as_u64().unwrap_or(0);
    assert!(open > 0, "an empty prints folder stops the job at a question: {st}");
    assert!(shown.iter().any(|l| l == "Continue"), "{shown:?}");
    let wl = e.exec("job.worklist", json!({ "status": "pending" }));
    let q = wl["items"].as_array().into_iter().flatten().find(|i| i["kind"] == "question").cloned().expect("a pending question");
    let choice = q["choices"].as_array().and_then(|c| c.iter().find_map(Value::as_str)).map(str::to_string);
    match choice {
        Some(choice) => {
            assert!(click_button(&mut e, &choice), "the answer button {choice}: {:?}", labels(&mut e));
        }
        None => {
            assert!(click_button(&mut e, "Type answer"));
            panic!("a typed answer isn't driven here: {q}");
        }
    }
    let after = e.exec("job.status", json!({}));
    assert!(after["answers"]["by_you"].as_u64().unwrap_or(0) >= 1, "the click answered it: {after}");

    // Its report.
    assert!(e.click("Report"));
    assert!(e.shows("Whole report"), "{:?}", labels(&mut e));
    let _ = std::fs::remove_dir_all(&prints);
}
