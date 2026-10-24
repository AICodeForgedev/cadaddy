//! The Job palette (MUST 6): a dockable panel showing the guided job's worklist,
//! evidence, actions/choices, and progress. Shows questions, findings and actions
//! from the unified worklist with evidence crops and inline answer controls.

use egui::RichText;
use serde_json::{Value, json};

use crate::CadApp;
use crate::theme::Tokens;

pub const DIALOG: &str = "guidedjob";

/// Which tab the palette shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Tab {
    #[default]
    Worklist,
    Progress,
    Findings,
    Rules,
}

#[derive(Clone, Default)]
struct State {
    tab: Tab,
    status: Option<Value>,
    worklist: Option<Value>,
    findings: Option<Value>,
    rules: Option<Value>,
    /// Filter: "all", "pending", "resolved", "info".
    filter: String,
    /// The answer being typed for a free-form question.
    answer_text: String,
    /// The question id being answered.
    answering: Option<String>,
    /// Last error message.
    error: Option<String>,
}

fn id() -> egui::Id {
    egui::Id::new("job_palette_state")
}

fn state(ctx: &egui::Context) -> State {
    ctx.data_mut(|d| d.get_temp::<State>(id()).unwrap_or_default())
}

fn set_state(ctx: &egui::Context, s: State) {
    ctx.data_mut(|d| d.insert_temp(id(), s));
}

pub fn show(app: &mut CadApp, ui: &mut egui::Ui) {
    let t = Tokens::get();
    let mut st = state(ui.ctx());
    let mut action: Option<Value> = None;

    // Tab bar.
    ui.horizontal(|ui| {
        for (tab, label) in [(Tab::Worklist, "Worklist"), (Tab::Progress, "Progress"), (Tab::Findings, "Findings"), (Tab::Rules, "Rules")] {
            let selected = st.tab == tab;
            let text =
                if selected { RichText::new(crate::i18n::t(label)).color(t.accent) } else { RichText::new(crate::i18n::t(label)).color(t.text_dim) };
            if ui.selectable_label(selected, text).clicked() {
                st.tab = tab;
            }
        }
        if ui.button(crate::i18n::t("Refresh")).clicked() {
            st.status = None;
            st.worklist = None;
            st.findings = None;
            st.rules = None;
        }
    });
    ui.separator();

    // Lazy-load data.
    if st.status.is_none() {
        st.status = app.session.execute("job.status", &json!({})).ok();
    }

    match st.tab {
        Tab::Worklist => show_worklist(app, ui, &mut st, &t, &mut action),
        Tab::Progress => show_progress(ui, &st, &t),
        Tab::Findings => show_findings(app, ui, &mut st, &t),
        Tab::Rules => show_rules(app, ui, &mut st, &t),
    }

    // Execute deferred actions.
    if let Some(act) = action
        && let Some(cmd) = act.get("command").and_then(Value::as_str)
    {
        let params = act.get("params").cloned().unwrap_or(json!({}));
        match app.session.execute(cmd, &params) {
            Ok(_) => {
                // Invalidate cache after action.
                st.status = None;
                st.worklist = None;
                st.findings = None;
                st.error = None;
            }
            Err(e) => {
                st.error = Some(e.to_string());
            }
        }
    }

    if let Some(err) = &st.error {
        ui.colored_label(egui::Color32::RED, err);
    }

    set_state(ui.ctx(), st);
}

fn show_worklist(app: &mut CadApp, ui: &mut egui::Ui, st: &mut State, t: &Tokens, action: &mut Option<Value>) {
    if st.worklist.is_none() {
        let filter = if st.filter.is_empty() || st.filter == "all" { json!({}) } else { json!({"status": st.filter}) };
        st.worklist = app.session.execute("job.worklist", &filter).ok();
    }

    // Filter bar.
    ui.horizontal(|ui| {
        for label in ["all", "pending", "resolved", "info"] {
            let selected = st.filter == label || (label == "all" && st.filter.is_empty());
            if ui.selectable_label(selected, label).clicked() {
                st.filter = label.to_string();
                st.worklist = None; // Re-fetch with new filter.
            }
        }
    });
    ui.separator();

    // Summary line.
    if let Some(wl) = &st.worklist {
        let pending = wl.get("pending").and_then(Value::as_u64).unwrap_or(0);
        let total = wl.get("total").and_then(Value::as_u64).unwrap_or(0);
        ui.label(format!("{pending} pending / {total} total"));
        ui.separator();
    }

    // Items.
    egui::ScrollArea::vertical().show(ui, |ui| {
        let items = st.worklist.as_ref().and_then(|w| w.get("items")).and_then(Value::as_array).cloned().unwrap_or_default();

        if items.is_empty() {
            ui.label(crate::i18n::t("No items."));
            return;
        }

        for item in &items {
            let item_id = item.get("id").and_then(Value::as_str).unwrap_or("?");
            let kind = item.get("kind").and_then(Value::as_str).unwrap_or("?");
            let stage = item.get("stage").and_then(Value::as_str).unwrap_or("?");
            let summary = item.get("summary").and_then(Value::as_str).unwrap_or("");
            let status = item.get("status").and_then(Value::as_str).unwrap_or("?");
            let has_undo = item.get("has_undo").and_then(Value::as_bool).unwrap_or(false);

            ui.group(|ui| {
                ui.horizontal(|ui| {
                    // Kind badge.
                    let badge_color = match kind {
                        "question" => t.accent,
                        "finding" => egui::Color32::from_rgb(220, 160, 40),
                        _ => t.text_dim,
                    };
                    ui.colored_label(badge_color, RichText::new(kind).strong().small());
                    ui.colored_label(t.text_dim, RichText::new(stage).small());

                    // Status badge.
                    let status_color = match status {
                        "pending" => egui::Color32::from_rgb(220, 160, 40),
                        "resolved" => egui::Color32::from_rgb(80, 180, 80),
                        _ => t.text_dim,
                    };
                    ui.colored_label(status_color, RichText::new(status).small());
                });

                ui.label(summary);

                // Evidence (values).
                if let Some(ev) = item.get("evidence").filter(|v| !v.is_null()) {
                    if let Some(vals) = ev.get("values") {
                        ui.label(RichText::new(serde_json::to_string_pretty(vals).unwrap_or_default()).monospace().small());
                    }
                    if let Some(sheet) = ev.get("sheet").and_then(Value::as_str) {
                        ui.label(RichText::new(format!("Sheet: {sheet}")).small());
                    }
                }

                // Choices / answer buttons.
                if status == "pending" {
                    let choices = item.get("choices").and_then(Value::as_array).cloned().unwrap_or_default();
                    if !choices.is_empty() {
                        ui.horizontal_wrapped(|ui| {
                            for choice in &choices {
                                if let Some(text) = choice.as_str()
                                    && ui.small_button(text).clicked()
                                {
                                    *action = Some(json!({
                                        "command": "job.answer",
                                        "params": {"id": item_id, "answer": text},
                                    }));
                                }
                            }
                        });
                    }

                    // Free-form answer.
                    if choices.is_empty() || st.answering.as_deref() == Some(item_id) {
                        ui.horizontal(|ui| {
                            let is_active = st.answering.as_deref() == Some(item_id);
                            if !is_active && ui.small_button("Type answer").clicked() {
                                st.answering = Some(item_id.to_string());
                                st.answer_text.clear();
                            }
                            if is_active {
                                ui.text_edit_singleline(&mut st.answer_text);
                                if ui.small_button("Submit").clicked() && !st.answer_text.is_empty() {
                                    *action = Some(json!({
                                        "command": "job.answer",
                                        "params": {"id": item_id, "answer": st.answer_text.clone()},
                                    }));
                                    st.answering = None;
                                    st.answer_text.clear();
                                }
                            }
                        });
                    }
                }

                // Undo button.
                if status == "resolved" && has_undo && ui.small_button("Undo").clicked() {
                    *action = Some(json!({
                        "command": "job.undo",
                        "params": {"id": item_id},
                    }));
                }
            });
        }
    });
}

fn show_progress(ui: &mut egui::Ui, st: &State, t: &Tokens) {
    let Some(status) = &st.status else {
        ui.label("No job running.");
        return;
    };

    let progress = status.get("progress").and_then(Value::as_f64).unwrap_or(0.0);
    let done = status.get("done").and_then(Value::as_bool).unwrap_or(false);
    let current = status.get("current_stage").and_then(Value::as_str).unwrap_or("—");

    ui.label(RichText::new(if done { "Complete" } else { "In progress" }).strong());
    ui.add(egui::ProgressBar::new(progress as f32).text(format!("{:.0}%", progress * 100.0)));
    if !done {
        ui.label(format!("Current: {current}"));
    }
    ui.separator();

    // Stage list.
    if let Some(stages) = status.get("stages").and_then(Value::as_array) {
        for stage in stages {
            let name = stage.get("stage").and_then(Value::as_str).unwrap_or("?");
            let st_status = stage.get("status").and_then(Value::as_str).unwrap_or("?");
            let unanswered = stage.get("unanswered").and_then(Value::as_u64).unwrap_or(0);

            let color = match st_status {
                "Done" => egui::Color32::from_rgb(80, 180, 80),
                "Error" => egui::Color32::RED,
                "Paused" => egui::Color32::from_rgb(220, 160, 40),
                "Running" => t.accent,
                _ => t.text_dim,
            };

            ui.horizontal(|ui| {
                ui.colored_label(color, RichText::new(st_status).small().monospace());
                ui.label(name);
                if unanswered > 0 {
                    ui.colored_label(egui::Color32::from_rgb(220, 160, 40), RichText::new(format!("{unanswered} pending")).small());
                }
            });
        }
    }

    // Summary stats.
    ui.separator();
    let open_q = status.get("open_questions").and_then(Value::as_u64).unwrap_or(0);
    let findings = status.get("findings").and_then(Value::as_u64).unwrap_or(0);
    let wl_pending = status.get("worklist_pending").and_then(Value::as_u64).unwrap_or(0);
    let rules = status.get("rules_learned").and_then(Value::as_u64).unwrap_or(0);

    ui.label(format!("Open questions: {open_q} | Findings: {findings} | Worklist pending: {wl_pending} | Rules: {rules}"));
}

fn show_findings(app: &mut CadApp, ui: &mut egui::Ui, st: &mut State, t: &Tokens) {
    if st.findings.is_none() {
        // Findings come from job.worklist filtered to kind=finding.
        st.findings = app.session.execute("job.worklist", &json!({})).ok().map(|v| {
            let items = v
                .get("items")
                .and_then(Value::as_array)
                .map(|arr| arr.iter().filter(|i| i.get("kind").and_then(Value::as_str) == Some("finding")).cloned().collect::<Vec<_>>())
                .unwrap_or_default();
            json!({"items": items})
        });
    }

    let items = st.findings.as_ref().and_then(|f| f.get("items")).and_then(Value::as_array).cloned().unwrap_or_default();

    if items.is_empty() {
        ui.label("No findings.");
        return;
    }

    ui.label(RichText::new(format!("{} finding(s)", items.len())).strong());
    ui.separator();

    egui::ScrollArea::vertical().show(ui, |ui| {
        for item in &items {
            let summary = item.get("summary").and_then(Value::as_str).unwrap_or("");
            let stage = item.get("stage").and_then(Value::as_str).unwrap_or("?");
            let status = item.get("status").and_then(Value::as_str).unwrap_or("?");

            ui.group(|ui| {
                ui.horizontal(|ui| {
                    ui.colored_label(egui::Color32::from_rgb(220, 160, 40), RichText::new("finding").strong().small());
                    ui.colored_label(t.text_dim, RichText::new(stage).small());
                    ui.colored_label(t.text_dim, RichText::new(status).small());
                });
                ui.label(summary);
            });
        }
    });
}

fn show_rules(app: &mut CadApp, ui: &mut egui::Ui, st: &mut State, t: &Tokens) {
    if st.rules.is_none() {
        st.rules = app.session.execute("job.rules", &json!({})).ok();
    }

    let rules = st.rules.as_ref().and_then(|r| r.get("rules")).and_then(Value::as_array).cloned().unwrap_or_default();

    if rules.is_empty() {
        ui.label("No rules learned yet.");
        return;
    }

    ui.label(RichText::new(format!("{} rule(s)", rules.len())).strong());
    ui.separator();

    egui::ScrollArea::vertical().show(ui, |ui| {
        for rule in &rules {
            let key = rule.get("key").and_then(Value::as_str).unwrap_or("?");
            let value = rule.get("value").and_then(Value::as_str).unwrap_or("?");
            let from = rule.get("from_question").and_then(Value::as_str).unwrap_or("?");

            ui.group(|ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(key).monospace().strong());
                    ui.label("→");
                    ui.label(RichText::new(value).monospace());
                });
                ui.colored_label(t.text_dim, RichText::new(format!("from {from}")).small());
            });
        }
    });
}
