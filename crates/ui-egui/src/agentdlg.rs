//! Agent safety rails in the app ([`cadaddy_engine::agent`]): the question when an agent wants to
//! save over a file or close with changes (Allow runs it, Deny doesn't), and Agent Activity
//! (Window menu): the change journal of agent requests with Select for what each changed, and
//! the dry run and confirmation switches.

use egui::{RichText, vec2};
use serde_json::{Value, json};

use crate::CadApp;
use crate::theme::Tokens;

pub const DIALOG: &str = "agentlog";

/// The question for an agent's call waiting for the user (drawn every frame while one waits).
pub fn confirm_prompt(app: &mut CadApp, ctx: &egui::Context) {
    let Some(p) = app.session.agent.pending.clone() else { return };
    let mut answer: Option<bool> = None;
    egui::Window::new(crate::tl!("An agent asks"))
        .id(egui::Id::new("agent_confirm"))
        .collapsible(false)
        .resizable(false)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .show(ctx, |ui| {
            ui.label(RichText::new(capitalised(&p.question)).strong());
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button(crate::tl!("Allow")).clicked() {
                    answer = Some(true);
                }
                if ui.button(crate::tl!("Deny")).clicked() {
                    answer = Some(false);
                }
            });
        });
    if let Some(allowed) = answer {
        answer_pending(app, allowed);
    }
}

fn capitalised(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

/// The user's answer to the waiting call: allowed, it runs now (as the user's own).
pub fn answer_pending(app: &mut CadApp, allowed: bool) {
    let Some(p) = app.session.agent.pending.take() else { return };
    app.session.agent.answered = Some((p.question.clone(), allowed));
    if allowed {
        let mut params = p.params.clone();
        if let Some(o) = params.as_object_mut() {
            o.insert("confirmed".into(), json!(true));
        }
        if let Err(e) = app.run(&p.command, params) {
            app.set_status(e);
        }
    } else {
        app.session.echo(crate::tf!("Not allowed: {question}", question = p.question));
    }
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let t = Tokens::get();
    let mut select: Option<Vec<String>> = None;
    let (mut dry, mut confirm) = (app.session.agent.dry_run, app.session.agent.confirm);
    let mut clear = false;
    egui::Window::new(crate::tl!("Agent Activity")).id(egui::Id::new("agent_activity")).open(open).default_size(vec2(460.0, 360.0)).show(ctx, |ui| {
        ui.checkbox(&mut confirm, crate::tl!("Ask me before an agent saves over a file or closes with changes"));
        ui.checkbox(&mut dry, crate::tl!("Dry run: agents' commands change nothing"));
        ui.separator();
        let entries: Vec<_> = app.session.agent.journal.iter().rev().cloned().collect();
        if entries.is_empty() {
            ui.label(RichText::new(crate::tl!("No agent requests yet.")).color(t.text_dim));
        }
        egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
            for e in &entries {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("#{} {}", e.id, e.label)).strong());
                    ui.label(RichText::new(&e.drawing).color(t.text_dim));
                });
                ui.horizontal(|ui| {
                    ui.label(crate::tf!(
                        "{commands} commands · {added} added · {removed} removed · {changed} changed",
                        commands = e.commands,
                        added = e.counts[0],
                        removed = e.counts[1],
                        changed = e.counts[2]
                    ));
                    let what: Vec<String> = e.added.iter().chain(&e.changed).cloned().collect();
                    if !what.is_empty()
                        && ui.small_button(crate::tl!("Select")).on_hover_text(crate::tl!("Select what it added and changed")).clicked()
                    {
                        select = Some(what);
                    }
                });
                ui.separator();
            }
        });
        if !entries.is_empty() && ui.button(crate::tl!("Clear")).clicked() {
            clear = true;
        }
    });
    // Switches as the user's own commands (an agent can't turn the question off).
    if confirm != app.session.agent.confirm {
        let _ = app.run("agent.confirm", json!({ "on": confirm }));
    }
    if dry != app.session.agent.dry_run {
        let _ = app.run("agent.dryrun", json!({ "on": dry }));
    }
    if clear {
        let _ = app.run("agent.journal", json!({ "clear": true }));
    }
    if let Some(hs) = select {
        let _ = app.run("select", json!({ "handles": hs.into_iter().map(Value::String).collect::<Vec<_>>() }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_engine::Session;

    #[test]
    fn an_agents_close_with_changes_waits_for_the_user() {
        let mut app = CadApp::new(Session::new(), crate::Services::default());
        app.run("line", json!({"points": [[0, 0], [5, 0]]})).unwrap();
        // As the control channel calls it.
        app.session.agent.from_agent = true;
        let e = app.run("close", json!({})).unwrap_err();
        app.session.agent.from_agent = false;
        assert!(e.contains("Waiting for the user"), "{e}");
        assert_eq!(app.session.docs.len(), 1, "still open");
        // The question draws; Deny keeps the drawing, and the agent can read the answer.
        let ctx = egui::Context::default();
        ctx.run_ui(egui::RawInput::default(), |ui| confirm_prompt(&mut app, ui.ctx())).textures_delta.clear();
        answer_pending(&mut app, false);
        assert_eq!(app.session.docs.len(), 1);
        let j = app.run("agent.journal", json!({})).unwrap();
        assert_eq!(j["answered"]["allowed"], false);
        // Asked again and allowed: it closes.
        app.session.agent.from_agent = true;
        let _ = app.run("close", json!({}));
        app.session.agent.from_agent = false;
        answer_pending(&mut app, true);
        assert_eq!(app.session.docs.len(), 0);
        // The activity window draws, with an entry.
        app.run("new", json!({})).unwrap();
        app.run("agent.begin", json!({"label": "Draw a box"})).unwrap();
        app.run("rectang", json!({"p1": [0, 0], "p2": [3, 3]})).unwrap();
        app.run("agent.end", json!({})).unwrap();
        let mut open = true;
        ctx.run_ui(egui::RawInput::default(), |ui| dialog(&mut app, ui.ctx(), &mut open)).textures_delta.clear();
        assert_eq!(app.session.agent.journal.len(), 1);
    }
}
