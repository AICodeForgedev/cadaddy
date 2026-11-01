//! The Validate panel (Automation ▸ Validate, Q6117): the BIM model checked against its prints by
//! a connected agent. Scope (selection, a room, a level or the building), trades, the rules (with
//! this project's changes; each can be turned off), Start / Resume / Cancel, how far the run is,
//! the scores per trade, and the judged findings: a click zooms to the element and shows the
//! print crop beside the model drawn over it; Accept (a real error), Reject (a false alarm) or
//! Defer. A thin view over the engine's `validate`, `validate.status`, `validate.cancel`,
//! `validate.rules`, `validate.rules.set`, `validate.areas` and the review queue (`review.list`,
//! `review.next`, `review.resolve`, `review.defer`), read once per drawing revision.

use egui::{Color32, RichText};
use serde_json::{Value, json};

use crate::CadApp;
use crate::docking::Panel;
use crate::theme::Tokens;

/// The ribbon's Validate button (and Window ▸ Validate).
pub const COMMAND: &str = "ui.panel.validate";

/// The trades a run can take, as `validate.rules` names them, with their labels.
const TRADES: [(&str, &str); 6] = [
    ("architecture", "Architecture"),
    ("electrical", "Electrical"),
    ("mechanical", "Mechanical"),
    ("plumbing", "Plumbing"),
    ("fire", "Fire"),
    ("general", "General"),
];
/// Rules, areas and findings read at most (pages of the engine's lists).
const MAX_RULES: usize = 300;
const MAX_ROOMS: usize = 2_000;
const MAX_FINDINGS: usize = 200;
/// The largest side a crop is shown at, points.
const CROP_MAX: f32 = 220.0;
/// The largest picture file read for a crop (bytes).
const MAX_IMAGE_BYTES: u64 = 40 * 1024 * 1024;

/// VALIDATE from the ribbon or Window ▸ Validate opens the panel, floating beside the drawing
/// the first time.
pub fn open(app: &mut CadApp) -> Result<Value, String> {
    let panel = Panel::Validate;
    let first = !app.ui.docking.layout.contains(&panel) && !app.ui.docking.hidden.iter().any(|(p, _)| *p == panel);
    let r = crate::docking::command(app, false, &json!({ "operation": "open", "panel": panel.id() }))?;
    if !first {
        return Ok(r);
    }
    let area = app.canvas.rect.unwrap_or(egui::Rect::from_min_size(egui::pos2(220.0, 80.0), egui::vec2(1000.0, 800.0)));
    let (w, h) = (480.0, (area.height() - 40.0).clamp(260.0, 760.0));
    let rect = [area.right() - w - 16.0, area.top() + 24.0, w, h];
    crate::docking::command(app, false, &json!({ "operation": "float", "panel": panel.id(), "rect": rect }))
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Scope {
    Selection,
    Room,
    Level,
    #[default]
    Building,
}

impl Scope {
    const ALL: [Self; 4] = [Self::Selection, Self::Room, Self::Level, Self::Building];
    fn id(self) -> &'static str {
        match self {
            Self::Selection => "selection",
            Self::Room => "room",
            Self::Level => "level",
            Self::Building => "building",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Selection => crate::tl!("Selection"),
            Self::Room => crate::tl!("Current room"),
            Self::Level => crate::tl!("Level"),
            Self::Building => crate::tl!("Building"),
        }
    }
}

/// The person's choices (kept while the app runs).
#[derive(Clone, Debug)]
struct Choices {
    scope: Scope,
    room: String,
    level: String,
    trades: [bool; 6],
    /// Findings: the open ones, or every one.
    all: bool,
    selected: Option<String>,
    /// A finding was just picked: scroll its detail into view.
    reveal: bool,
}

impl Default for Choices {
    fn default() -> Self {
        Self { scope: Scope::Building, room: String::new(), level: String::new(), trades: [true; 6], all: false, selected: None, reveal: false }
    }
}

/// What the panel shows, read from the engine once per drawing revision.
#[derive(Clone, Debug, Default)]
struct View {
    key: (u64, u64, bool),
    /// (number or name, label) of each model room.
    rooms: Vec<(String, String)>,
    levels: Vec<String>,
    rules: Vec<Value>,
    status: Option<Value>,
    findings: Vec<Value>,
}

/// A finding's detail (`review.next {id}`), for the revision it was read at.
#[derive(Clone, Debug, Default)]
struct Detail {
    key: (u64, u64, String),
    item: Value,
}

fn choices_id() -> egui::Id {
    egui::Id::new("validate panel choices")
}

fn view_id() -> egui::Id {
    egui::Id::new("validate panel view")
}

fn detail_id() -> egui::Id {
    egui::Id::new("validate panel detail")
}

fn doc_key(app: &CadApp) -> (u64, u64) {
    app.session.state().map(|st| (st.uid, st.revision)).unwrap_or_default()
}

/// Every page of a paged engine list (`items` under `key`), up to `max`.
fn pages(app: &mut CadApp, cmd: &str, mut params: Value, key: &str, max: usize) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for _ in 0..64 {
        let Ok(r) = app.session.execute(cmd, &params) else { break };
        out.extend(r[key].as_array().into_iter().flatten().cloned());
        let next = r.get("next").or_else(|| r.get("next_cursor")).cloned().unwrap_or(Value::Null);
        let cursor = match &next {
            Value::Number(_) | Value::String(_) => next,
            Value::Object(o) => o.get("cursor").cloned().unwrap_or(Value::Null),
            _ => Value::Null,
        };
        if cursor.is_null() || out.len() >= max {
            break;
        }
        params["cursor"] = cursor;
    }
    out.truncate(max);
    out
}

fn read_view(app: &mut CadApp, key: (u64, u64, bool), all: bool) -> View {
    let mut v = View { key, ..View::default() };
    let areas = pages(app, "validate.areas", json!({"kind": "room", "limit": 500}), "areas", MAX_ROOMS);
    for a in &areas {
        let number = a["number"].as_str().or_else(|| a["name"].as_str()).unwrap_or_default().to_string();
        if number.is_empty() || v.rooms.iter().any(|(n, _)| *n == number) {
            continue;
        }
        let label = match (a["number"].as_str(), a["name"].as_str()) {
            (Some(n), Some(m)) => format!("{n} {m}"),
            _ => number.clone(),
        };
        let level = a["level"].as_str().unwrap_or_default();
        v.rooms.push((number, if level.is_empty() { label } else { format!("{label} ({level})") }));
    }
    for a in &areas {
        if let Some(l) = a["level"].as_str().filter(|l| !l.is_empty())
            && !v.levels.iter().any(|x| x == l)
        {
            v.levels.push(l.to_string());
        }
    }
    v.rules = pages(app, "validate.rules", json!({"detail": false, "limit": 50}), "rules", MAX_RULES);
    v.status = app.session.execute("validate.status", &json!({})).ok();
    let status = if all { "all" } else { "open" };
    v.findings = pages(app, "review.list", json!({"kind": "validation", "status": status, "limit": 50}), "items", MAX_FINDINGS)
        .into_iter()
        // The judge's findings (the bim.validate pre-screen's own items are in the review queue).
        .filter(|i| i["id"].as_str().is_some_and(|id| id.starts_with("RV-J")))
        .collect();
    v
}

/// The run's progress in words: "N of M areas, waiting for an agent".
fn progress(status: &Value, watched: bool) -> String {
    let n = |k: &str| status["areas"][k].as_u64().unwrap_or(0);
    let (done, total, issued, pending) = (n("done"), n("total"), n("issued"), n("pending"));
    let state = if status["cancelled"] == true {
        crate::tl!("cancelled")
    } else if total > 0 && done >= total {
        crate::tl!("done")
    } else if issued > 0 || (watched && pending > 0) {
        crate::tl!("judging")
    } else {
        crate::tl!("waiting for an agent")
    };
    crate::tf!("{done} of {total} areas, {state}", done = done, total = total, state = state)
}

fn pct(v: &Value) -> String {
    v.as_f64().filter(|x| x.is_finite()).map_or_else(|| "—".to_string(), |x| format!("{:.0}%", x * 100.0))
}

fn severity_color(t: &Tokens, s: &str) -> Color32 {
    match s {
        "high" => Color32::from_rgb(0xd0, 0x45, 0x3a),
        "medium" => t.warn,
        _ => t.text_dim,
    }
}

/// A button named in `ui.elements`, so agents and tests find it.
fn button(ui: &mut egui::Ui, label: &str, enabled: bool) -> egui::Response {
    let r = ui.add_enabled(enabled, egui::Button::new(label));
    crate::control::record_widget(ui, &r, label, "button", None);
    r
}

/// A line of text named in `ui.elements` (role "text").
fn said(ui: &mut egui::Ui, text: impl Into<egui::WidgetText>, plain: &str) {
    let r = ui.label(text);
    crate::control::record_widget(ui, &r, plain, "text", None);
}

/// What a click asked for, done once the panel is drawn.
enum Act {
    Start,
    Resume,
    Cancel,
    Send,
    DontSend,
    Rule(String, bool),
    Select(String),
    Answer(String, &'static str),
    Defer(String),
}

pub fn show(app: &mut CadApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let mut c: Choices = ctx.data(|d| d.get_temp(choices_id())).unwrap_or_default();
    let (uid, revision) = doc_key(app);
    let key = (uid, revision, c.all);
    let view = match ctx.data(|d| d.get_temp::<View>(view_id())) {
        Some(v) if v.key == key => v,
        _ => {
            let v = read_view(app, key, c.all);
            ctx.data_mut(|d| d.insert_temp(view_id(), v.clone()));
            v
        }
    };
    let t = Tokens::get();
    let mut act: Option<Act> = None;
    egui::ScrollArea::vertical().id_salt("dock-validate").auto_shrink([false, false]).show(ui, |ui| {
        egui::Frame::new().inner_margin(egui::Margin::symmetric(8, 6)).show(ui, |ui| {
            // Scores per trade.
            match &view.status {
                Some(st) => {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(crate::tl!("Scores")).strong());
                        ui.label(format!("{} {}", crate::tl!("Overall"), pct(&st["accuracy"]["overall"])));
                        if let Some(by) = st["accuracy"]["by_trade"].as_object() {
                            for (trade, score) in by {
                                let label = TRADES.iter().find(|(id, _)| id == trade).map_or(trade.as_str(), |(_, l)| *l);
                                ui.label(RichText::new(format!("{} {}", crate::i18n::t(label), pct(score))).color(t.text_dim));
                            }
                        }
                    });
                    let p = progress(st, app.session.events.on || app.validate_api.busy());
                    said(ui, p.as_str(), &p);
                    if let Some(fa) = st["review"]["false_alarm_rate"].as_f64() {
                        ui.label(RichText::new(crate::tf!("False alarms: {rate}", rate = pct(&json!(fa)))).color(t.text_dim));
                    }
                }
                None => {
                    let s = crate::tl!("No run yet: choose a scope and trades, then Start.");
                    said(ui, RichText::new(s).color(t.text_dim), s);
                }
            }
            ui.separator();
            // Scope.
            ui.horizontal(|ui| {
                ui.label(crate::tl!("Scope:"));
                egui::ComboBox::from_id_salt("validate scope").selected_text(c.scope.label()).show_ui(ui, |ui| {
                    for s in Scope::ALL {
                        ui.selectable_value(&mut c.scope, s, s.label());
                    }
                });
                match c.scope {
                    Scope::Room => {
                        if c.room.is_empty()
                            && let Some((n, _)) = view.rooms.first()
                        {
                            c.room = n.clone();
                        }
                        let shown = view.rooms.iter().find(|(n, _)| *n == c.room).map_or(c.room.clone(), |(_, l)| l.clone());
                        egui::ComboBox::from_id_salt("validate room").selected_text(shown).show_ui(ui, |ui| {
                            for (n, l) in &view.rooms {
                                ui.selectable_value(&mut c.room, n.clone(), l);
                            }
                        });
                    }
                    Scope::Level => {
                        if c.level.is_empty()
                            && let Some(l) = view.levels.first()
                        {
                            c.level = l.clone();
                        }
                        egui::ComboBox::from_id_salt("validate level").selected_text(c.level.clone()).show_ui(ui, |ui| {
                            for l in &view.levels {
                                ui.selectable_value(&mut c.level, l.clone(), l);
                            }
                        });
                    }
                    Scope::Selection | Scope::Building => {}
                }
            });
            // Trades.
            ui.horizontal_wrapped(|ui| {
                ui.label(crate::tl!("Trades:"));
                for (on, (_, label)) in c.trades.iter_mut().zip(TRADES) {
                    let label = crate::i18n::t(label);
                    let r = ui.checkbox(on, label);
                    crate::control::record_widget(ui, &r, label, "checkbox", Some(*on));
                }
            });
            // Rules.
            let enabled = view.rules.iter().filter(|r| r["enabled"] != false).count();
            let rules_title = crate::tf!("Rules ({on} of {all} on)", on = enabled, all = view.rules.len());
            let header = egui::CollapsingHeader::new(rules_title.as_str()).id_salt("validate rules").show(ui, |ui| {
                for r in &view.rules {
                    let id = r["id"].as_str().unwrap_or_default();
                    let trade = r["trade"].as_str().unwrap_or_default();
                    if !TRADES.iter().zip(c.trades).any(|((t, _), on)| on && *t == trade) {
                        continue;
                    }
                    let mut on = r["enabled"] != false;
                    ui.horizontal(|ui| {
                        let toggle = ui.checkbox(&mut on, RichText::new(id).monospace());
                        crate::control::record_widget(ui, &toggle, id, "checkbox", Some(on));
                        if toggle.changed() {
                            act = Some(Act::Rule(id.to_string(), on));
                        }
                        if r["source"] == "project" {
                            ui.label(RichText::new(crate::tl!("project")).small().color(t.accent));
                        }
                        let check = r["check"].as_str().unwrap_or_default();
                        ui.label(RichText::new(check).small().color(t.text_dim)).on_hover_text(check);
                    });
                }
            });
            crate::control::record_widget(ui, &header.header_response, &rules_title, "button", Some(header.body_returned.is_some()));
            // Run.
            let run = view.status.as_ref().and_then(|s| s["run"].as_str()).map(str::to_string);
            let left = view.status.as_ref().is_some_and(|s| s["areas"]["done"].as_u64().unwrap_or(0) < s["areas"]["total"].as_u64().unwrap_or(0));
            let cancelled = view.status.as_ref().is_some_and(|s| s["cancelled"] == true);
            ui.horizontal(|ui| {
                if button(ui, crate::tl!("Start"), true).clicked() {
                    act = Some(Act::Start);
                }
                if button(ui, crate::tl!("Resume"), run.is_some() && (left || cancelled)).clicked() {
                    act = Some(Act::Resume);
                }
                if button(ui, crate::tl!("Cancel"), run.is_some() && left && !cancelled).clicked() {
                    act = Some(Act::Cancel);
                }
            });
            // Judged by the Anthropic API (Options ▸ Automation), when no agent is attached.
            match crate::validateapi::section(app, ui) {
                Some(crate::validateapi::Choice::Send) => act = Some(Act::Send),
                Some(crate::validateapi::Choice::Cancel) => act = Some(Act::DontSend),
                None => {}
            }
            ui.separator();
            // Findings.
            ui.horizontal(|ui| {
                let n = crate::tf!("Findings ({n})", n = view.findings.len());
                said(ui, RichText::new(n.as_str()).strong(), &n);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let r = ui.checkbox(&mut c.all, crate::tl!("Show answered"));
                    crate::control::record_widget(ui, &r, crate::tl!("Show answered"), "checkbox", Some(c.all));
                });
            });
            if view.findings.is_empty() {
                ui.label(RichText::new(crate::tl!("No findings to review.")).color(t.text_dim));
            }
            for f in &view.findings {
                let id = f["id"].as_str().unwrap_or_default();
                let title = f["title"].as_str().unwrap_or_default();
                let picked = c.selected.as_deref() == Some(id);
                ui.horizontal(|ui| {
                    let sev = title.split_whitespace().next().unwrap_or_default();
                    let color =
                        view.rules.iter().find(|r| r["id"] == sev).and_then(|r| r["severity"].as_str()).map_or(t.text_dim, |s| severity_color(&t, s));
                    let (dot, _) = ui.allocate_exact_size(egui::vec2(10.0, 14.0), egui::Sense::hover());
                    ui.painter().circle_filled(dot.center(), 4.0, color);
                    let r = ui.selectable_label(picked, title);
                    crate::control::record_widget(ui, &r, title, "button", Some(picked));
                    if r.clicked() {
                        act = Some(Act::Select(id.to_string()));
                    }
                });
            }
            if let Some(id) = c.selected.clone()
                && let Some(item) = detail(app, &ctx, &id)
            {
                ui.separator();
                let shown = finding(ui, &ctx, &id, &item, &mut act);
                if std::mem::take(&mut c.reveal) {
                    ui.scroll_to_rect(shown, Some(egui::Align::BOTTOM));
                }
            }
        });
    });
    match act {
        Some(Act::Start) => start(app, &c, None),
        Some(Act::Resume) => start(app, &c, view.status.as_ref()),
        Some(Act::Cancel) => {
            crate::validateapi::cancel(app);
            let r = app.run("validate.cancel", json!({ "run": view.status.as_ref().map(|s| s["run"].clone()) }));
            report(app, r);
        }
        Some(Act::Send) => crate::validateapi::confirm(app),
        Some(Act::DontSend) => crate::validateapi::cancel(app),
        Some(Act::Rule(id, on)) => {
            let r = app.run("validate.rules.set", json!({"id": id, "enabled": on, "reason": "Validate panel"}));
            report(app, r);
        }
        Some(Act::Select(id)) => {
            c.selected = Some(id.clone());
            c.reveal = true;
            if let Some(item) = detail(app, &ctx, &id) {
                zoom(app, &item["detail"]["zoom"]);
            }
        }
        Some(Act::Answer(id, answer)) => {
            let r = app.run("review.resolve", json!({"id": id, "answer": answer, "author": "user"}));
            report(app, r);
        }
        Some(Act::Defer(id)) => {
            let r = app.run("review.defer", json!({"id": id, "reason": "Deferred in the Validate panel"}));
            report(app, r);
        }
        None => {}
    }
    ctx.data_mut(|d| d.insert_temp(choices_id(), c));
}

fn report(app: &mut CadApp, r: Result<Value, String>) {
    match r {
        Ok(v) => {
            if let Some(n) = v["note"].as_str().or_else(|| v["waiting"].as_str()) {
                app.set_status(crate::i18n::t(n).to_string());
            }
        }
        Err(e) => app.set_status(e),
    }
}

/// Start a run of the panel's scope and trades, or `resume` the run of a status as it was asked.
fn start(app: &mut CadApp, c: &Choices, resume: Option<&Value>) {
    let mut p = match resume {
        Some(st) => {
            let mut p = st["scope"].clone();
            if !p.is_object() {
                p = json!({"scope": "building"});
            }
            if st["trades"].as_array().is_some_and(|a| !a.is_empty()) {
                p["trades"] = st["trades"].clone();
            }
            if st["rules"].as_array().is_some_and(|a| !a.is_empty()) {
                p["rules"] = st["rules"].clone();
            }
            p
        }
        None => {
            let mut p = json!({"scope": c.scope.id()});
            match c.scope {
                Scope::Room => p["room"] = json!(c.room),
                Scope::Level => p["level"] = json!(c.level),
                Scope::Selection | Scope::Building => {}
            }
            // Every trade: no list (the run takes every rule that applies).
            if c.trades.iter().any(|on| !on) {
                p["trades"] = json!(TRADES.iter().zip(c.trades).filter(|(_, on)| *on).map(|((id, _), _)| *id).collect::<Vec<_>>());
            }
            p
        }
    };
    if let Some(o) = p.as_object_mut() {
        o.retain(|_, v| !v.is_null());
    }
    let r = app.run("validate", p);
    // With the Anthropic API judge on and no agent attached, the panel judges the run itself.
    if let Ok(v) = &r
        && crate::validateapi::wanted(app)
        && v["pending"].as_u64().unwrap_or(0) > 0
        && let Some(run) = v["run"].as_str()
    {
        let run = run.to_string();
        crate::validateapi::begin(app, &run);
        return;
    }
    report(app, r);
}

/// The selected finding's detail, read once per drawing revision.
fn detail(app: &mut CadApp, ctx: &egui::Context, id: &str) -> Option<Value> {
    let (uid, revision) = doc_key(app);
    let key = (uid, revision, id.to_string());
    if let Some(d) = ctx.data(|d| d.get_temp::<Detail>(detail_id()))
        && d.key == key
    {
        return Some(d.item);
    }
    let item = app.session.execute("review.next", &json!({"id": id})).ok()?.get("item").cloned()?;
    ctx.data_mut(|d| d.insert_temp(detail_id(), Detail { key, item: item.clone() }));
    Some(item)
}

/// Zoom to a finding's element (selected), when it has one in the drawing.
fn zoom(app: &mut CadApp, target: &Value) {
    let Some(h) = target["handle"].as_str().filter(|h| !h.is_empty()) else { return };
    let known = app.session.doc().is_ok_and(|d| d.model.iter().any(|e| e.handle.hex().eq_ignore_ascii_case(h)));
    if !known {
        return;
    }
    if let Err(e) = app.run("zoom", json!({ "mode": "object", "handles": [h] })) {
        app.set_status(e);
    }
    let _ = app.run("select", json!({ "handles": [h] }));
}

/// A picture file as a texture (cached by path and size), for the crops.
fn texture(ctx: &egui::Context, path: &str) -> Option<egui::TextureHandle> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > MAX_IMAGE_BYTES {
        return None;
    }
    let id = egui::Id::new(("validate crop", path, meta.len()));
    if let Some(t) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return Some(t);
    }
    let bytes = std::fs::read(path).ok()?;
    let img = image::load_from_memory(&bytes).ok()?.to_rgba8();
    let size = [usize::try_from(img.width()).ok()?, usize::try_from(img.height()).ok()?];
    let tex =
        ctx.load_texture(format!("validate-crop:{path}"), egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw()), egui::TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, tex.clone()));
    Some(tex)
}

fn crop(ui: &mut egui::Ui, ctx: &egui::Context, caption: &str, path: Option<&str>) {
    ui.vertical(|ui| {
        said(ui, RichText::new(caption).small().strong(), caption);
        match path.and_then(|p| texture(ctx, p)) {
            Some(tex) => {
                let s = tex.size_vec2();
                let k = (CROP_MAX / s.x.max(s.y).max(1.0)).min(1.0);
                ui.add(egui::Image::new((tex.id(), s * k)).fit_to_exact_size(s * k));
            }
            None => {
                ui.label(RichText::new(crate::tl!("(no picture)")).small().color(Tokens::get().text_dim));
            }
        }
    });
}

/// The selected finding: what the judge expected and found, the print crop beside the model
/// crop, and the answers.
/// Returns the answers' row, to scroll to.
fn finding(ui: &mut egui::Ui, ctx: &egui::Context, id: &str, item: &Value, act: &mut Option<Act>) -> egui::Rect {
    let t = Tokens::get();
    let d = &item["detail"];
    ui.label(RichText::new(item["title"].as_str().unwrap_or_default()).strong());
    let row = |ui: &mut egui::Ui, name: &str, v: &Value| {
        let s = match v {
            Value::String(s) => s.clone(),
            Value::Null => return,
            other => other.to_string(),
        };
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(name).color(t.text_dim));
            ui.label(s);
        });
    };
    row(ui, crate::tl!("Issue:"), &d["issue"]);
    row(ui, crate::tl!("Expected:"), &d["expected"]);
    row(ui, crate::tl!("Found:"), &d["found"]);
    row(ui, crate::tl!("Confidence:"), &json!(pct(&d["confidence"])));
    let evidence: Vec<&Value> = d["evidence"].as_array().into_iter().flatten().take(4).collect();
    for e in evidence {
        if let Some(what) = e["what"].as_str().filter(|w| !w.is_empty()) {
            ui.label(RichText::new(format!("{} — {what}", e["sheet"].as_str().unwrap_or_default())).small());
        }
        // A print crop beside the model drawn over the same crop; a picture of the model alone
        // (model/…) is shown as the model.
        let image = e["image"].as_str();
        ui.horizontal_top(|ui| {
            if image.is_some_and(|i| i.replace('\\', "/").contains("/prints/")) {
                crop(ui, ctx, crate::tl!("Print"), image);
                crop(ui, ctx, crate::tl!("Model"), e["model"].as_str());
            } else {
                crop(ui, ctx, crate::tl!("Model"), image);
            }
        });
    }
    ui.horizontal(|ui| {
        if button(ui, crate::tl!("Accept"), true).on_hover_text(crate::tl!("It is a real error")).clicked() {
            *act = Some(Act::Answer(id.to_string(), "accept"));
        }
        if button(ui, crate::tl!("Reject"), true).on_hover_text(crate::tl!("A false alarm")).clicked() {
            *act = Some(Act::Answer(id.to_string(), "reject"));
        }
        if button(ui, crate::tl!("Defer"), true).on_hover_text(crate::tl!("Leave it for a field decision")).clicked() {
            *act = Some(Act::Defer(id.to_string()));
        }
    })
    .response
    .rect
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_says_how_far_and_what_it_waits_for() {
        let st = |done: u64, issued: u64, pending: u64| json!({"areas": {"done": done, "issued": issued, "pending": pending, "total": done + issued + pending}});
        assert_eq!(progress(&st(1, 0, 3), false), "1 of 4 areas, waiting for an agent");
        assert_eq!(progress(&st(1, 1, 2), false), "1 of 4 areas, judging");
        assert_eq!(progress(&st(1, 0, 3), true), "1 of 4 areas, judging");
        assert_eq!(progress(&st(4, 0, 0), false), "4 of 4 areas, done");
        let mut c = st(1, 0, 3);
        c["cancelled"] = json!(true);
        assert_eq!(progress(&c, false), "1 of 4 areas, cancelled");
        assert_eq!(pct(&json!(0.786)), "79%");
        assert_eq!(pct(&Value::Null), "—");
    }
}
