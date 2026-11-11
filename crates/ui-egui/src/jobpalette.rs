//! The Job panel (Automation ▸ Job, Window ▸ Job, Tools ▸ Guided Job ▸ Start Job...; Q6266): a
//! guided job from a person's side. With no job (or a finished one and New job): the start form —
//! the prints folder, the goal, whether the job stops at each question or runs unattended, where to
//! keep its drawing — and Start. While its stages run (off the UI thread, Q6269): the stage, how
//! far it is, the line it is on, and Cancel. Then its worklist (the questions it waits on, with
//! their answer controls, findings and actions), its progress per stage, findings, the rules it
//! learned and its report; Continue runs it on to its next question, Run unattended takes every
//! default. A thin view over `job.start`, `job.next`, `job.answer`, `job.set_mode`, `job.status`,
//! `job.worklist`, `job.rules`, `job.report` and `job.undo`.

use std::sync::{Arc, Mutex};

use egui::RichText;
use serde_json::{Value, json};

use crate::CadApp;
use crate::docking::Panel;
use crate::theme::Tokens;

/// The ribbon's Job button (and Window ▸ Job).
pub const COMMAND: &str = "ui.panel.job";
/// The engine's command that starts a job: from a menu or typed alone it opens the start form.
pub const START: &str = "job.start";

/// The goals a job can take (`job.start`'s `goal`), with their labels; the first is the default.
const GOALS: [(&str, &str); 6] = [
    ("estimate", "Estimate"),
    ("takeoff", "Takeoff"),
    ("bim", "BIM model"),
    ("cdy", "Drawing from the prints"),
    ("clash", "Clash check"),
    ("sequence", "Clash check and sequence"),
];
/// The most of a report shown as text, characters.
const MAX_REPORT_TEXT: usize = 20_000;

/// Window ▸ Job, the ribbon's Job, START from a menu or typed alone: opens the panel, floating
/// beside the drawing the first time.
pub fn open(app: &mut CadApp) -> Result<Value, String> {
    let panel = Panel::Job;
    let first = !app.ui.docking.layout.contains(&panel) && !app.ui.docking.hidden.iter().any(|(p, _)| *p == panel);
    let r = crate::docking::command(app, false, &json!({ "operation": "open", "panel": panel.id() }))?;
    if !first {
        return Ok(r);
    }
    let area = app.canvas.rect.unwrap_or(egui::Rect::from_min_size(egui::pos2(220.0, 80.0), egui::vec2(1000.0, 800.0)));
    let (w, h) = (460.0, (area.height() - 40.0).clamp(260.0, 760.0));
    let rect = [area.right() - w - 16.0, area.top() + 24.0, w, h];
    crate::docking::command(app, false, &json!({ "operation": "float", "panel": panel.id(), "rect": rect }))
}

/// Which tab the palette shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Tab {
    #[default]
    Worklist,
    Progress,
    Findings,
    Rules,
    Report,
}

/// The start form's choices.
#[derive(Clone, Default)]
struct Form {
    prints: String,
    /// Index into [`GOALS`].
    goal: usize,
    /// Run unattended (every question with a default takes it).
    auto: bool,
    /// Where to keep the job's drawing (empty: a temporary one).
    checkpoint: String,
}

#[derive(Clone, Default)]
struct State {
    tab: Tab,
    /// The drawing (uid, revision) the cached replies were read at.
    key: (u64, u64),
    status: Option<Value>,
    worklist: Option<Value>,
    findings: Option<Value>,
    rules: Option<Value>,
    report: Option<Value>,
    /// Filter: "all", "pending", "resolved", "info".
    filter: String,
    /// The answer being typed for a free-form question.
    answer_text: String,
    /// The question id being answered.
    answering: Option<String>,
    form: Form,
    /// The start form is shown though a finished job is there (New job).
    new_job: bool,
    /// The last command's error (written when work that ran in the background ends).
    error: Arc<Mutex<Option<String>>>,
}

impl State {
    fn forget(&mut self) {
        self.status = None;
        self.worklist = None;
        self.findings = None;
        self.rules = None;
        self.report = None;
    }
}

fn id() -> egui::Id {
    egui::Id::new("job_palette_state")
}

/// The palette's state (the job's worklist, findings and report), moved out of egui's memory
/// for the frame, not copied (Q6482).
fn state(ctx: &egui::Context) -> State {
    crate::kept::take::<State>(ctx, id()).unwrap_or_default()
}

fn set_state(ctx: &egui::Context, s: State) {
    crate::kept::put(ctx, id(), s);
}

fn doc_key(app: &CadApp) -> (u64, u64) {
    app.session.state().map(|st| (st.uid, st.revision)).unwrap_or_default()
}

fn set_error(st: &State, e: Option<String>) {
    if let Ok(mut g) = st.error.lock() {
        *g = e;
    }
}

/// Run a job command; a long one runs off the UI thread and the panel reads the job again when
/// it ends. Its error shows in the panel.
fn run(app: &mut CadApp, st: &mut State, command: &str, params: Value) {
    st.forget();
    set_error(st, None);
    let error = Arc::clone(&st.error);
    app.run_background(command, params, move |_, r| {
        if let (Err(e), Ok(mut g)) = (r, error.lock()) {
            *g = Some(e);
        }
    });
}

fn button(ui: &mut egui::Ui, label: &str) -> bool {
    let r = ui.button(label);
    crate::control::record_widget(ui, &r, label, "button", None);
    r.clicked()
}

fn small_button(ui: &mut egui::Ui, label: &str) -> bool {
    let r = ui.small_button(label);
    crate::control::record_widget(ui, &r, label, "button", None);
    r.clicked()
}

/// A line of text, seen by `ui.elements` too.
fn text(ui: &mut egui::Ui, text: impl Into<RichText>, plain: &str) {
    let r = ui.label(text.into());
    crate::control::record_widget(ui, &r, plain, "text", None);
}

pub fn show(app: &mut CadApp, ui: &mut egui::Ui) {
    let t = Tokens::get();
    let mut st = state(ui.ctx());
    let mut action: Option<Value> = None;

    // Work running: what it is and how far, with Cancel. The window shows a copy of the drawing
    // meanwhile, so nothing is read from it; the job is read again when the work ends.
    if let Some(running) = crate::busy::running(app).map(str::to_string) {
        st.forget();
        show_running(app, ui, &running, &t);
        set_state(ui.ctx(), st);
        return;
    }
    let key = doc_key(app);
    if key != st.key {
        st.forget();
        st.key = key;
    }
    if st.status.is_none() {
        st.status = app.session.execute("job.status", &json!({})).ok();
    }
    let job = st.status.as_ref().filter(|s| s.get("job_id").is_some()).cloned();
    if job.is_none() || (job.as_ref().is_some_and(replaceable) && st.new_job) {
        show_form(app, ui, &mut st, &t, job.as_ref());
        show_error(ui, &st);
        set_state(ui.ctx(), st);
        return;
    }
    let job = job.unwrap_or_default();
    st.new_job = false;

    show_summary(app, ui, &mut st, &job, &t);
    ui.separator();

    // Tab bar.
    ui.horizontal_wrapped(|ui| {
        for (tab, label) in
            [(Tab::Worklist, "Worklist"), (Tab::Progress, "Progress"), (Tab::Findings, "Findings"), (Tab::Rules, "Rules"), (Tab::Report, "Report")]
        {
            let selected = st.tab == tab;
            let text =
                if selected { RichText::new(crate::i18n::t(label)).color(t.accent) } else { RichText::new(crate::i18n::t(label)).color(t.text_dim) };
            let r = ui.selectable_label(selected, text);
            crate::control::record_widget(ui, &r, crate::i18n::t(label), "tab", Some(selected));
            if r.clicked() {
                st.tab = tab;
            }
        }
        if ui.button(crate::i18n::t("Refresh")).clicked() {
            st.forget();
        }
    });
    ui.separator();

    match st.tab {
        Tab::Worklist => show_worklist(app, ui, &mut st, &t, &mut action),
        Tab::Progress => show_progress(ui, &st, &t),
        Tab::Findings => show_findings(app, ui, &mut st, &t),
        Tab::Rules => show_rules(app, ui, &mut st, &t),
        Tab::Report => show_report(app, ui, &mut st, &t),
    }

    // Deferred actions: answers and undo.
    if let Some(act) = action
        && let Some(cmd) = act.get("command").and_then(Value::as_str)
    {
        let params = act.get("params").cloned().unwrap_or(json!({}));
        run(app, &mut st, cmd, params);
    }
    show_error(ui, &st);
    set_state(ui.ctx(), st);
}

fn show_error(ui: &mut egui::Ui, st: &State) {
    let err = st.error.lock().ok().and_then(|g| g.clone());
    if let Some(err) = err {
        text(ui, RichText::new(&err).color(egui::Color32::RED), &err);
    }
}

/// The work running now: the job's stage, how far, the line it is on, and Cancel.
fn show_running(app: &mut CadApp, ui: &mut egui::Ui, running: &str, t: &Tokens) {
    let status = crate::busy::status(app).unwrap_or_default();
    let head = crate::tf!("Running {command}", command = running);
    text(ui, RichText::new(&head).strong(), &head);
    if let Some(stage) = status.get("current_stage").and_then(Value::as_str) {
        let line = crate::tf!("Stage: {stage}", stage = stage);
        text(ui, &line, &line);
    }
    if let Some(p) = status.get("progress").and_then(Value::as_f64) {
        let p = if p.is_finite() { p.clamp(0.0, 1.0) } else { 0.0 };
        ui.add(egui::ProgressBar::new(p as f32).text(format!("{:.0}%", p * 100.0)));
    }
    if let Some(line) = status.get("line").and_then(Value::as_str) {
        ui.colored_label(t.text_dim, RichText::new(line).small());
    }
    if let Some(c) = status.get("cancel").and_then(Value::as_str) {
        ui.colored_label(t.text_dim, c);
    } else if button(ui, crate::tl!("Cancel")) {
        crate::busy::cancel(app);
    }
}

/// A job a new one may take the place of: it is done, or it stopped on an error.
fn replaceable(job: &Value) -> bool {
    job.get("done").and_then(Value::as_bool) == Some(true) || job.get("error").is_some_and(|e| !e.is_null())
}

/// No job yet (or New job): where the prints are, what the job is for, and how it runs.
fn show_form(app: &mut CadApp, ui: &mut egui::Ui, st: &mut State, t: &Tokens, old: Option<&Value>) {
    ui.label(RichText::new(crate::tl!("Start a guided job")).strong());
    ui.colored_label(
        t.text_dim,
        crate::tl!("The job reads a set of prints (PDF or DWG sheets) and builds the drawing, the model and the takeoff from them, asking where it is unsure."),
    );
    ui.separator();
    egui::Grid::new("job start form").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
        ui.label(crate::tl!("Prints folder"));
        ui.horizontal(|ui| {
            let r = ui.add(egui::TextEdit::singleline(&mut st.form.prints).desired_width(220.0));
            crate::control::record_widget(ui, &r, crate::tl!("Prints folder"), "textbox", None);
            if button(ui, crate::tl!("Browse...")) {
                match app.services.pick_folder.as_ref().map(|f| f()) {
                    Some(Some(dir)) => st.form.prints = dir,
                    Some(None) => {}
                    None => set_error(st, Some(crate::tl!("There is no folder dialog here: type the prints folder.").to_string())),
                }
            }
        });
        ui.end_row();

        ui.label(crate::tl!("Goal"));
        let current = GOALS.get(st.form.goal).map_or("", |g| g.1);
        let combo = egui::ComboBox::from_id_salt("job goal").selected_text(crate::i18n::t(current)).show_ui(ui, |ui| {
            for (i, (_, label)) in GOALS.iter().enumerate() {
                let r = ui.selectable_value(&mut st.form.goal, i, crate::i18n::t(label));
                crate::control::record_widget(ui, &r, crate::i18n::t(label), "option", Some(st.form.goal == i));
            }
        });
        crate::control::record_widget(ui, &combo.response, crate::tl!("Goal"), "combobox", None);
        ui.end_row();

        ui.label(crate::tl!("Questions"));
        ui.vertical(|ui| {
            for (auto, label) in [(false, crate::tl!("Stop at each question")), (true, crate::tl!("Run unattended (take each default)"))] {
                let r = ui.radio_value(&mut st.form.auto, auto, label);
                crate::control::record_widget(ui, &r, label, "radio", Some(st.form.auto == auto));
            }
        });
        ui.end_row();

        ui.label(crate::tl!("Keep the job's drawing"));
        ui.horizontal(|ui| {
            let r = ui.add(egui::TextEdit::singleline(&mut st.form.checkpoint).hint_text(crate::tl!("optional: a new .cdy")).desired_width(220.0));
            crate::control::record_widget(ui, &r, crate::tl!("Keep the job's drawing"), "textbox", None);
            if button(ui, crate::tl!("Save As..."))
                && let Some(Some(path)) = app.services.pick_save.as_ref().map(|f| f("Job.cdy"))
            {
                st.form.checkpoint = path;
            }
        });
        ui.end_row();
    });
    ui.separator();
    ui.horizontal(|ui| {
        if button(ui, crate::tl!("Start")) {
            match start_params(&st.form) {
                Ok(mut params) => {
                    // New job after another is starting over, never going on from the other's
                    // checkpoint (a first Start does go on from a job of these prints cut off).
                    if old.is_some() {
                        params["resume"] = json!(false);
                    }
                    // A drawing holds one job until it is done: after one that stopped on an
                    // error, the new job goes in a new drawing (the old one is kept as it is).
                    let stopped = old.is_some_and(|j| j.get("done").and_then(Value::as_bool) != Some(true));
                    let fresh = if stopped { app.run("new", json!({})).map(|_| ()) } else { Ok(()) };
                    match fresh {
                        Ok(()) => {
                            st.new_job = false;
                            st.tab = Tab::Worklist;
                            run(app, st, START, params);
                        }
                        Err(e) => set_error(st, Some(e)),
                    }
                }
                Err(e) => set_error(st, Some(e)),
            }
        }
        if old.is_some() && button(ui, crate::tl!("Back to the job")) {
            st.new_job = false;
        }
    });
    if old.is_some_and(|j| j.get("done").and_then(Value::as_bool) != Some(true)) {
        ui.colored_label(t.text_dim, crate::tl!("The job that stopped stays in its drawing; the new one opens in a new drawing."));
    }
}

/// `job.start`'s parameters from the form.
fn start_params(form: &Form) -> Result<Value, String> {
    let prints = form.prints.trim();
    if prints.is_empty() {
        return Err(crate::tl!("Choose the prints folder first.").to_string());
    }
    let goal = GOALS.get(form.goal).map_or("estimate", |g| g.0);
    let mut p = json!({ "pdfs": prints, "goal": goal, "auto": form.auto });
    let checkpoint = form.checkpoint.trim();
    if !checkpoint.is_empty() {
        p["checkpoint"] = json!(checkpoint);
    }
    Ok(p)
}

/// The job in one look: its goal, where it is, how far, and what a person can do next.
fn show_summary(app: &mut CadApp, ui: &mut egui::Ui, st: &mut State, job: &Value, t: &Tokens) {
    let goal = job.get("goal").and_then(Value::as_str).unwrap_or("?");
    let done = job.get("done").and_then(Value::as_bool).unwrap_or(false);
    let progress = job.get("progress").and_then(Value::as_f64).filter(|p| p.is_finite()).unwrap_or(0.0).clamp(0.0, 1.0);
    let open = job.get("open_questions").and_then(Value::as_u64).unwrap_or(0);
    let auto = job.pointer("/answers/mode").and_then(Value::as_str) == Some("auto");
    let label = GOALS.iter().find(|g| g.0.eq_ignore_ascii_case(goal)).map_or(goal, |g| g.1);
    let title = crate::tf!("Job: {goal}", goal = crate::i18n::t(label));
    text(ui, RichText::new(&title).strong(), &title);
    let error = job.pointer("/error/message").and_then(Value::as_str);
    let state = if done {
        job.get("status").and_then(Value::as_str).map_or_else(|| crate::tl!("Done").to_string(), |s| s.replace('_', " "))
    } else if let Some(e) = error {
        let stage = job.pointer("/error/stage").and_then(Value::as_str).unwrap_or("—");
        crate::tf!("Stopped at {stage}: {error}", stage = stage, error = e)
    } else if open > 0 {
        crate::tf!("Waiting for {n} answer(s)", n = open)
    } else {
        let stage = job.get("current_stage").and_then(Value::as_str).unwrap_or("—");
        crate::tf!("Stopped at {stage}", stage = stage)
    };
    if error.is_some() {
        text(ui, RichText::new(&state).color(egui::Color32::RED), &state);
    } else {
        text(ui, &state, &state);
    }
    ui.add(egui::ProgressBar::new(progress as f32).text(format!("{:.0}%", progress * 100.0)));
    ui.horizontal_wrapped(|ui| {
        if !done && error.is_none() {
            if button(ui, crate::tl!("Continue")) {
                run(app, st, "job.next", json!({}));
            }
            if !auto && button(ui, crate::tl!("Run unattended")) {
                run(app, st, "job.set_mode", json!({ "auto": true }));
            }
        }
        if replaceable(job) && button(ui, crate::tl!("New job")) {
            st.new_job = true;
        }
    });
    if open > 0 && st.tab != Tab::Worklist {
        ui.colored_label(t.accent, crate::tl!("Answer its questions on the Worklist tab."));
    }
}

/// The report: how it ended, its warnings, the answers' totals, and the whole report as text.
fn show_report(app: &mut CadApp, ui: &mut egui::Ui, st: &mut State, t: &Tokens) {
    if st.report.is_none() {
        st.report = app.session.execute("job.report", &json!({})).ok();
    }
    let Some(r) = st.report.clone() else {
        ui.label(crate::tl!("No report yet."));
        return;
    };
    egui::ScrollArea::vertical().id_salt("job report").show(ui, |ui| {
        if let Some(status) = r.get("status").and_then(Value::as_str) {
            ui.label(RichText::new(status.replace('_', " ")).strong());
        }
        let warnings = r.get("warnings").and_then(Value::as_array).cloned().unwrap_or_default();
        if !warnings.is_empty() {
            ui.label(RichText::new(crate::tf!("Warnings ({n})", n = warnings.len())).strong());
            for w in &warnings {
                let head = w.get("warning").and_then(Value::as_str).or_else(|| w.as_str()).unwrap_or("?");
                let detail = w.get("detail").and_then(Value::as_str).unwrap_or("");
                ui.colored_label(egui::Color32::from_rgb(220, 160, 40), head);
                if !detail.is_empty() {
                    ui.colored_label(t.text_dim, RichText::new(detail).small());
                }
            }
            ui.separator();
        }
        if let Some(totals) = r.get("totals").and_then(Value::as_object) {
            ui.horizontal_wrapped(|ui| {
                for (k, v) in totals {
                    ui.label(format!("{k}: {v}"));
                }
            });
        }
        if let Some(nb) = r.get("not_built").and_then(Value::as_str).filter(|s| !s.is_empty()) {
            ui.label(crate::tf!("Not built: {sheets}", sheets = nb));
        }
        let whole = egui::CollapsingHeader::new(crate::tl!("Whole report")).id_salt("job whole report").show(ui, |ui| {
            let mut text = serde_json::to_string_pretty(&r).unwrap_or_default();
            if text.len() > MAX_REPORT_TEXT {
                let cut = (0..=MAX_REPORT_TEXT).rev().find(|i| text.is_char_boundary(*i)).unwrap_or(0);
                text.truncate(cut);
                text.push_str("\n…");
            }
            ui.label(RichText::new(text).monospace().small());
        });
        crate::control::record_widget(ui, &whole.header_response, crate::tl!("Whole report"), "button", Some(whole.body_returned.is_some()));
    });
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
                                    && small_button(ui, text)
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
                            if !is_active && small_button(ui, crate::tl!("Type answer")) {
                                st.answering = Some(item_id.to_string());
                                st.answer_text.clear();
                            }
                            if is_active {
                                ui.text_edit_singleline(&mut st.answer_text);
                                if small_button(ui, crate::tl!("Submit")) && !st.answer_text.is_empty() {
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
                if status == "resolved" && has_undo && small_button(ui, crate::tl!("Undo")) {
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
                    ui.label("->");
                    ui.label(RichText::new(value).monospace());
                });
                ui.colored_label(t.text_dim, RichText::new(format!("from {from}")).small());
            });
        }
    });
}
