//! The ribbon's Automation tab (Q6117), driven as a person does through the control channel:
//! the tab sits after Output with Validate then Takeoff; Validate opens its panel, which starts,
//! cancels and resumes a run, lists the judged findings and takes a person's answer; Takeoff on a
//! made-up BIM model fills its table window. `AUTOMATION_SHOTS=<folder>` also saves headless
//! renders of the tab and the panel there (skipped without a GPU adapter).

use crate::e2e_common::{E2e, folder};
use serde_json::{Value, json};

/// A made-up 30' × 12' plan with an office and a storage room, built into a model.
fn modelled(e: &mut E2e) {
    let p = |x: f64, y: f64| json!([x * 12.0, y * 12.0]);
    let rect = |a: (f64, f64), b: (f64, f64)| json!({ "vertices": [p(a.0, a.1), p(b.0, a.1), p(b.0, b.1), p(a.0, b.1)], "closed": true });
    e.exec("layer.new", json!({ "name": "A-WALL", "current": true }));
    e.exec("pline", rect((0.0, 0.0), (30.0, 12.0)));
    e.exec("pline", rect((0.5, 0.5), (29.5, 11.5)));
    e.exec("line", json!({ "points": [p(15.0, 0.5), p(15.0, 11.5)] }));
    e.exec("line", json!({ "points": [p(15.4, 0.5), p(15.4, 11.5)] }));
    e.exec("layer.new", json!({ "name": "A-AREA-IDEN", "current": true }));
    for (x, name, n) in [(6.0, "OFFICE", "101"), (21.0, "STORAGE", "102")] {
        e.exec("text", json!({ "at": p(x, 6.0), "text": name, "height": 8 }));
        e.exec("text", json!({ "at": p(x, 4.5), "text": n, "height": 6 }));
    }
    e.exec("bspec.draft", json!({ "store": true }));
    let b = e.exec("model.build", json!({}));
    assert!(b["walls"].as_u64().unwrap_or(0) >= 4, "{b}");
}

fn element(e: &mut E2e, label: &str) -> Option<Value> {
    e.elements().into_iter().find(|x| x["label"].as_str() == Some(label))
}

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

/// Click the first control whose label starts with `prefix`.
fn click_prefix(e: &mut E2e, prefix: &str) -> bool {
    let Some(x) = e.elements().into_iter().find(|x| x["label"].as_str().is_some_and(|l| l.starts_with(prefix))) else { return false };
    let [x, y, w, h] = rect(&x);
    e.ok("ui.click", json!({ "x": x + w / 2.0, "y": y + h / 2.0, "space": "screen" }));
    true
}

fn shows_prefix(e: &mut E2e, prefix: &str) -> bool {
    e.elements().iter().any(|x| x["label"].as_str().is_some_and(|l| l.starts_with(prefix)))
}

fn docked(e: &mut E2e, panel: &str) -> bool {
    e.ok("ui.inspect", json!({}))["ui"]["docking"].to_string().contains(panel)
}

#[test]
fn the_automation_tab_follows_output_with_validate_then_takeoff() {
    let mut e = E2e::new();
    let output = element(&mut e, "Output").expect("the Output tab");
    let automation = element(&mut e, "Automation").expect("the Automation tab");
    assert!(rect(&automation)[0] > rect(&output)[0], "Automation comes after Output: {automation} {output}");
    assert!(e.click("Automation"));
    assert_eq!(e.app.ui.ribbon_tab, "Automation");
    let validate = element(&mut e, "Validate").expect("the Validate button");
    let takeoff = element(&mut e, "Takeoff").expect("the Takeoff button");
    assert!(rect(&validate)[0] < rect(&takeoff)[0], "Validate first, then Takeoff");
    // The Electrical workspace shows it too.
    e.exec("setvar", json!({ "name": "WSCURRENT", "value": "Electrical" }));
    e.pump();
    assert!(e.shows("Automation"), "the Electrical workspace keeps the Automation tab");
}

#[test]
fn takeoff_on_a_model_fills_the_table_window() {
    let mut e = E2e::new();
    modelled(&mut e);
    assert!(e.click("Automation"));
    assert!(e.click("Takeoff"), "the Takeoff button");
    assert_eq!(e.dialog().as_deref(), Some("takeoff.table"));
    let table = e.app.takeoff_table.clone().expect("the takeoff's table");
    assert_eq!(table["source"], "model", "{table}");
    let rows = table["rows"].as_array().cloned().unwrap_or_default();
    assert!(!rows.is_empty(), "{table}");
    // The table on screen: its header and the first line's cells.
    for h in ["Level", "Trade", "Type", "Qty"] {
        assert!(e.shows(h), "column {h}");
    }
    let first: Vec<String> = rows[0].as_array().unwrap().iter().map(|c| c.as_str().unwrap_or_default().to_string()).collect();
    for c in first.iter().filter(|c| !c.is_empty()).take(4) {
        assert!(e.shows(c), "cell {c} of {first:?}");
    }
    assert!(e.shows("Export") && e.shows("Refresh"));
    // Without a model it is the 2D takeoff.
    let mut e = E2e::new();
    e.exec("layer.new", json!({ "name": "E-POWR", "current": true }));
    e.exec("line", json!({ "points": [[0, 0], [120, 0]] }));
    e.menu("automation.takeoff");
    assert_eq!(e.app.takeoff_table.as_ref().map(|t| t["source"].clone()), Some(json!("drawing")));
    assert!(e.shows("E-POWR"));
}

#[test]
fn the_validate_panel_runs_and_grades_a_validation() {
    let mut e = E2e::new();
    modelled(&mut e);
    let dir = folder("automation-validate");
    e.exec("saveas", json!({ "path": dir.join("Sample Plan.cdy") }));
    assert!(e.click("Automation"));
    assert!(e.click("Validate"), "the Validate button");
    assert!(docked(&mut e, "validate"), "the panel is open");
    for l in ["Start", "Resume", "Cancel", "Electrical", "Mechanical"] {
        assert!(e.shows(l), "{l} in the panel");
    }
    assert!(shows_prefix(&mut e, "Rules ("), "the rules list");

    // Start: a run of the building, waiting for an agent (nobody watches events here).
    assert!(click_button(&mut e, "Start"));
    let st = e.exec("validate.status", json!({}));
    let run = st["run"].as_str().expect("a run").to_string();
    let total = st["areas"]["total"].as_u64().unwrap_or(0);
    assert!(total >= 2, "{st}");
    assert!(
        e.shows(&format!("0 of {total} areas, waiting for an agent")),
        "{:?}",
        e.elements().iter().map(|x| x["label"].clone()).collect::<Vec<_>>()
    );

    // Cancel, then Resume the same run.
    assert!(click_button(&mut e, "Cancel"));
    assert_eq!(e.exec("validate.status", json!({}))["cancelled"], true);
    assert!(e.shows(&format!("0 of {total} areas, cancelled")));
    assert!(click_button(&mut e, "Resume"));
    let st = e.exec("validate.status", json!({}));
    assert_eq!(st["run"], json!(run));
    assert!(st.get("cancelled").is_none(), "{st}");

    // A fake agent judges one area: one finding it is unsure of.
    let next = e.exec("validate.next", json!({ "run": run }));
    let area = next["area"]["id"].as_str().expect("an area").to_string();
    let dir_p = std::path::PathBuf::from(next["packet"]["dir"].as_str().unwrap());
    let manifest: Value = serde_json::from_str(&std::fs::read_to_string(dir_p.join("manifest.json")).unwrap()).unwrap();
    let image = ["prints", "model"]
        .iter()
        .flat_map(|k| manifest[*k].as_array().cloned().unwrap_or_default())
        .find_map(|x| x["file"].as_str().map(str::to_string))
        .unwrap_or_else(|| panic!("a picture in {manifest}"));
    let rule = next["rules"].as_array().and_then(|r| r.first()).map(|r| r["id"].as_str().or(r.as_str()).unwrap_or_default().to_string()).unwrap();
    let finding = json!({"rule": rule, "verdict": "unsure", "issue": "The wall type cannot be read on the plan", "expected": "a tagged wall type",
        "found": "no tag", "evidence": [{"image": image, "sheet": "A-101", "what": "the room's walls"}], "confidence": 0.5});
    e.exec("validate.report", json!({ "run": run, "area": area, "findings": [finding] }));
    e.pump();
    assert!(e.shows(&format!("1 of {total} areas, waiting for an agent")));
    assert!(e.shows("Findings (1)"));

    // The finding: click it, the evidence beside it, then Accept.
    assert!(click_prefix(&mut e, &format!("{rule} unsure")), "the finding in the list");
    for l in ["Accept", "Reject", "Defer", "Model"] {
        assert!(e.shows(l), "{l} for the finding");
    }
    let accept = e.elements().into_iter().find(|x| x["label"] == "Accept" && x["role"] == "button").unwrap();
    assert_eq!(accept["visible"], true, "the picked finding's answers scroll into view: {accept}");
    assert!(click_button(&mut e, "Accept"));
    let st = e.exec("validate.status", json!({ "run": run }));
    assert_eq!(st["review"]["accepted"], 1, "{st}");
    assert!(e.shows("Findings (0)"), "an answered finding leaves the open list");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Headless renders of the tab and the panel, for looking at (`AUTOMATION_SHOTS=<folder>`).
#[test]
fn renders_of_the_tab_and_the_panel() {
    let Some(out) = std::env::var_os("AUTOMATION_SHOTS").map(std::path::PathBuf::from) else { return };
    std::fs::create_dir_all(&out).unwrap();
    let probe = std::panic::catch_unwind(|| render(|_| {}, (64, 48)));
    if probe.is_err() {
        eprintln!("automation_tab: no GPU adapter; no renders");
        return;
    }
    render(|_| {}, (1280, 800)).save(out.join("automation-tab.png")).unwrap();
    render_clicking(
        |e| {
            modelled(e);
            let dir = folder("automation-shot");
            e.exec("saveas", json!({ "path": dir.join("Sample Plan.cdy") }));
            e.menu("ui.panel.validate");
            click_button(e, "Start");
            let next = e.exec("validate.next", json!({}));
            let area = next["area"]["id"].as_str().unwrap_or_default().to_string();
            let dir_p = std::path::PathBuf::from(next["packet"]["dir"].as_str().unwrap_or_default());
            let manifest: Value = serde_json::from_str(&std::fs::read_to_string(dir_p.join("manifest.json")).unwrap_or_default()).unwrap_or_default();
            let image = ["prints", "model"]
                .iter()
                .flat_map(|k| manifest[*k].as_array().cloned().unwrap_or_default())
                .find_map(|x| x["file"].as_str().map(str::to_string))
                .unwrap_or_default();
            let rule = next["rules"].as_array().and_then(|r| r.first()).and_then(|r| r["id"].as_str().or(r.as_str())).unwrap_or_default().to_string();
            e.exec(
                "validate.report",
                json!({"area": area, "findings": [{"rule": rule, "verdict": "unsure", "issue": "The wall type cannot be read on the plan",
                    "expected": "a tagged wall type", "found": "no tag", "evidence": [{"image": image, "sheet": "A-101", "what": "the room's walls"}], "confidence": 0.5}]}),
            );
            e.pump();
        },
        Some("G-PRESENT unsure"),
        (1280, 800),
    )
    .save(out.join("validate-panel.png"))
    .unwrap();
}

/// The whole window on the Automation tab after `setup`, rendered with the wgpu renderer; then
/// a click on the control whose label starts with `click` (if any), as a person's pointer.
fn render(setup: impl FnOnce(&mut E2e), (w, h): (u32, u32)) -> image::RgbaImage {
    render_clicking(setup, None, (w, h))
}

fn render_clicking(setup: impl FnOnce(&mut E2e), click: Option<&str>, (w, h): (u32, u32)) -> image::RgbaImage {
    let mut e = E2e::new();
    e.app.ui.ribbon = true;
    setup(&mut e);
    e.app.ui.ribbon_tab = "Automation".into();
    #[allow(clippy::cast_precision_loss)]
    let size = egui::vec2(w as f32, h as f32);
    let mut harness = egui_kittest::Harness::builder().with_size(size).wgpu().build_ui_state(
        |ui, app: &mut cadaddy_ui_egui::CadApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        e.app,
    );
    harness.run_steps(6);
    if let Some(prefix) = click {
        let ctx = harness.ctx.clone();
        let (req, _reply) = cadaddy_ui_egui::control::ControlRequest::new("ui.elements", json!({}));
        let found = match cadaddy_ui_egui::control::handle(harness.state_mut(), &ctx, &req) {
            cadaddy_ui_egui::control::Outcome::Done(v) => {
                v["result"].as_array().into_iter().flatten().find(|x| x["label"].as_str().is_some_and(|l| l.starts_with(prefix))).map(rect)
            }
            cadaddy_ui_egui::control::Outcome::Screenshot { .. } => None,
        };
        if let Some([x, y, w, h]) = found {
            #[allow(clippy::cast_possible_truncation)]
            let pos = egui::pos2((x + w / 2.0) as f32, (y + h / 2.0) as f32);
            harness.event(egui::Event::PointerMoved(pos));
            harness.run_steps(1);
            for pressed in [true, false] {
                harness.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
                harness.run_steps(1);
            }
            harness.run_steps(6);
        }
    }
    harness.render().unwrap()
}
