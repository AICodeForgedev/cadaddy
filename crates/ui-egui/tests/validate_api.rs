//! Validate's direct judge through the Anthropic API (Q6118), driven as a person does: off by
//! default; Options ▸ Automation turns it on, names the model and holds the key field; an agent
//! can't turn it on; the key never shows in the preferences. With it on and no agent attached,
//! the Validate panel's Start shows what will be sent, waits for "Send to Anthropic", then judges
//! every area through a made-up transport (no network) and keeps the findings.

mod e2e_common;

use std::sync::{Arc, Mutex};

use cadaddy_engine::cmd::validate_api as api;
use e2e_common::{E2e, folder};
use serde_json::{Value, json};

const KEY: &str = "sk-ant-api03-TESTKEY_abcdefghijklmnopqrstuvwxyz0123456789";

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

fn rect(v: &Value) -> [f64; 4] {
    let r: Vec<f64> = v["rect"].as_array().into_iter().flatten().filter_map(Value::as_f64).collect();
    [r.first().copied().unwrap_or(0.0), r.get(1).copied().unwrap_or(0.0), r.get(2).copied().unwrap_or(0.0), r.get(3).copied().unwrap_or(0.0)]
}

fn click_button(e: &mut E2e, label: &str) -> bool {
    let Some(x) = e.elements().into_iter().find(|x| x["label"].as_str() == Some(label) && x["role"] == "button") else { return false };
    let [x, y, w, h] = rect(&x);
    e.ok("ui.click", json!({ "x": x + w / 2.0, "y": y + h / 2.0, "space": "screen" }));
    true
}

fn labels(e: &mut E2e) -> Vec<String> {
    e.elements().iter().filter_map(|x| x["label"].as_str().map(str::to_string)).collect()
}

/// The made-up Messages API: answers every request with a pass for each rule the request's tool
/// allows, citing its first picture; keeps the bodies.
struct FakeApi {
    bodies: Mutex<Vec<String>>,
}

impl api::Transport for FakeApi {
    fn post(&self, _key: &api::ApiKey, body: &str) -> Result<api::Reply, api::SendError> {
        self.bodies.lock().unwrap().push(body.to_string());
        let v: Value = serde_json::from_str(body).unwrap();
        let item = &v["tools"][0]["input_schema"]["properties"]["findings"]["items"]["properties"];
        let image = item["evidence"]["items"]["properties"]["image"]["enum"][0].clone();
        let findings: Vec<Value> = item["rule"]["enum"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|r| json!({"rule": r, "verdict": "pass", "issue": "checked", "evidence": [{"image": image, "what": "the area"}], "confidence": 0.9}))
            .collect();
        let reply = json!({"type": "message", "stop_reason": "tool_use", "content": [{"type": "tool_use", "id": "t", "name": "report_findings", "input": {"findings": findings}}]});
        Ok(api::Reply { status: 200, body: reply.to_string(), retry_after: None })
    }
}

fn fake_judge(e: &mut E2e) -> Arc<FakeApi> {
    let fake = Arc::new(FakeApi { bodies: Mutex::new(Vec::new()) });
    let j = &mut e.app.validate_api;
    j.transport = fake.clone();
    j.key = Arc::new(|| Ok(Some(api::ApiKey::parse(KEY).unwrap())));
    j.sleep = Arc::new(|_| {});
    fake
}

#[test]
fn the_judge_is_off_by_default_and_only_the_user_turns_it_on() {
    let mut e = E2e::new();
    let o = e.exec("options", json!({}));
    assert_eq!((o["apiJudge"].clone(), o["apiJudgeModel"].clone()), (json!(false), json!("claude-opus-5-5")), "{o}");
    assert!(!e.app.validate_api.on);
    // An agent can't turn it on or change the model.
    e.app.session.agent.from_agent = true;
    for p in [json!({"apiJudge": true}), json!({"apiJudgeModel": "claude-sonnet-5-5"})] {
        let r = e.app.run("options", p.clone());
        assert_eq!(r.unwrap_err(), "Only the user can change apiJudge or apiJudgeModel (Options ▸ Automation), not an agent.", "{p}");
    }
    e.app.session.agent.from_agent = false;
    assert!(!e.app.validate_api.on);
    // The user can; a bad model name is refused.
    assert!(e.app.run("options", json!({"apiJudgeModel": "bad model\n"})).is_err());
    let r = e.app.run("options", json!({"apiJudge": true, "apiJudgeModel": "claude-sonnet-5-5"})).unwrap();
    assert_eq!((r["apiJudge"].clone(), r["apiJudgeModel"].clone()), (json!(true), json!("claude-sonnet-5-5")));
    // Kept across restarts; a key is no preference.
    let prefs = e.app.prefs_json();
    let v: Value = serde_json::from_str(&prefs).unwrap();
    assert_eq!((v["apiJudge"].clone(), v["apiJudgeModel"].clone()), (json!(true), json!("claude-sonnet-5-5")));
    assert!(!prefs.contains("sk-ant") && !e.app.profile_json().contains("sk-ant"));
    let mut f = E2e::new();
    f.app.load_prefs(&prefs);
    assert!(f.app.validate_api.on);
    assert_eq!(f.app.validate_api.model, "claude-sonnet-5-5");
    // The key isn't an option at all.
    assert!(e.app.run("options", json!({"apiKey": KEY})).is_err());
    // Reset turns it off.
    e.app.run("options", json!({"reset": true})).unwrap();
    assert!(!e.app.validate_api.on);
    assert_eq!(e.app.validate_api.model, "claude-opus-5-5");
}

#[test]
fn the_options_automation_checkbox_turns_it_on() {
    let mut e = E2e::new();
    fake_judge(&mut e);
    e.app.ui.dialog = Some("options:automation".into());
    e.pump();
    let l = labels(&mut e);
    for want in ["Judge with Anthropic API", "Model", "API key"] {
        assert!(l.iter().any(|x| x == want), "{want}: {l:?}");
    }
    // The checkbox turns it on, through the OPTIONS form.
    let cb = e.elements().into_iter().find(|x| x["label"] == "Judge with Anthropic API").unwrap();
    assert_eq!(cb["selected"], json!(false), "{cb}");
    let [x, y, w, h] = rect(&cb);
    e.ok("ui.click", json!({ "x": x + w / 2.0, "y": y + h / 2.0, "space": "screen" }));
    assert!(e.app.validate_api.on, "the checkbox turned it on");
}

#[test]
fn the_panel_judges_a_run_through_the_api_after_the_user_says_send() {
    let mut e = E2e::new();
    modelled(&mut e);
    let dir = folder("validate-api");
    e.exec("saveas", json!({ "path": dir.join("Sample Plan.cdy") }));
    let fake = fake_judge(&mut e);
    e.app.validate_api.on = true;
    assert!(e.click("Automation"));
    assert!(e.click("Validate"), "the Validate button");
    assert!(click_button(&mut e, "Start"));
    let st = e.exec("validate.status", json!({}));
    let total = st["areas"]["total"].as_u64().unwrap_or(0);
    assert!(total >= 2, "{st}");
    // Nothing is sent before the user says so; the panel says what would go.
    assert!(fake.bodies.lock().unwrap().is_empty());
    let l = labels(&mut e);
    assert!(l.iter().any(|x| x == "Send to Anthropic") && l.iter().any(|x| x == "Don't send"), "{l:?}");
    let (areas, images, tokens) = e.app.validate_api.confirming().expect("waiting for the go");
    assert_eq!(areas as u64, total);
    assert!(images > 0 && tokens > 0);
    // Don't send: nothing goes out.
    assert!(click_button(&mut e, "Don't send"));
    assert!(e.app.validate_api.confirming().is_none());
    assert!(fake.bodies.lock().unwrap().is_empty());
    // Resume, then Send: every area is judged, one request each.
    assert!(click_button(&mut e, "Resume"));
    assert!(click_button(&mut e, "Send to Anthropic"));
    for _ in 0..400 {
        if !e.app.validate_api.busy() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
        e.frame();
    }
    assert!(!e.app.validate_api.busy());
    let st = e.exec("validate.status", json!({}));
    assert_eq!(st["areas"]["done"], json!(total), "{st} {:?}", e.app.validate_api.log);
    assert_eq!(fake.bodies.lock().unwrap().len() as u64, total, "one request per area");
    assert!(fake.bodies.lock().unwrap().iter().all(|b| !b.contains("TESTKEY")), "the key is never in a body");
    assert_eq!(e.app.validate_api.judged as u64, total);
    assert!(e.app.validate_api.log.iter().any(|l| l.starts_with("Done:")), "{:?}", e.app.validate_api.log);
    // An agent attached: the panel leaves the judging to it.
    e.app.session.events.on = true;
    assert!(!cadaddy_ui_egui::validateapi::wanted(&e.app));
    let _ = std::fs::remove_dir_all(&dir);
}
