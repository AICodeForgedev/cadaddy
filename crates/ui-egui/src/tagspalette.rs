//! The Element Tags palette (Window ▸ Element Tags, ELEMENTTAGS typed alone): what each element is
//! from the documents (`tags.list`) — every field with where it came from — filtered by what is
//! unresolved (untagged, tag not in the schedule, missing or double protection, two rules);
//! pick elements and set a field on all of them (`tags.set`) or accept them as they are
//! (`tags.accept`).

use egui::RichText;
use serde_json::{Value, json};

use crate::CadApp;

pub const DIALOG: &str = "elementtags";

/// What the palette keeps between frames.
#[derive(Clone, Default)]
struct State {
    issue: Option<String>,
    picked: Vec<String>,
    field: String,
    value: String,
    report: String,
}

fn issue_label(i: &str) -> &'static str {
    match i {
        "untagged" => crate::tl!("Untagged"),
        "tag_not_in_schedule" => crate::tl!("Tag not in schedule"),
        "missing_protection" => crate::tl!("Missing protection"),
        "double_protection" => crate::tl!("Double protection"),
        "no_conductor_note" => crate::tl!("No conductor note"),
        "two_rules_apply" => crate::tl!("Two rules apply"),
        "method_choice_open" => crate::tl!("Method choice open"),
        _ => crate::tl!("Other"),
    }
}

const ISSUES: [&str; 7] =
    ["untagged", "tag_not_in_schedule", "missing_protection", "double_protection", "no_conductor_note", "two_rules_apply", "method_choice_open"];

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let t = crate::theme::Tokens::get();
    let id = egui::Id::new("tags-palette-state");
    let mut st: State = ctx.data(|d| d.get_temp(id)).unwrap_or_default();
    let mut q = json!({ "limit": 1000 });
    match &st.issue {
        Some(i) => q["issue"] = json!(i),
        None => q["unresolved"] = json!(false),
    }
    let r = app.session.execute("tags.list", &q).unwrap_or(Value::Null);
    let list: Vec<Value> = r["tags"].as_array().cloned().unwrap_or_default();
    let mut act: Option<(&str, Value)> = None;
    egui::Window::new(crate::tl!("Element Tags"))
        .id(egui::Id::new("Element Tags"))
        .default_pos(crate::palette_pos(app))
        .open(open)
        .resizable(true)
        .default_size([440.0, 520.0])
        .show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                let shown = st.issue.as_deref().map_or(crate::tl!("All elements"), issue_label);
                egui::ComboBox::from_id_salt("tags-issue").selected_text(shown).show_ui(ui, |ui| {
                    ui.selectable_value(&mut st.issue, None, crate::tl!("All elements"));
                    for i in ISSUES {
                        let n = r["byIssue"][i].as_u64().unwrap_or(0);
                        ui.selectable_value(&mut st.issue, Some(i.to_string()), format!("{} ({n})", issue_label(i)));
                    }
                });
                ui.label(RichText::new(crate::tf!("{n} elements", n = r["total"].as_u64().unwrap_or(0))).small().color(t.text_dim));
            });
            ui.separator();
            egui::ScrollArea::vertical().id_salt("tags-list").max_height(300.0).show(ui, |ui| {
                if list.is_empty() {
                    ui.label(RichText::new(crate::tl!("No element tags: read the electrical plans first.")).color(t.text_dim));
                }
                for item in &list {
                    let tid = item["id"].as_str().unwrap_or_default().to_string();
                    let mut on = st.picked.contains(&tid);
                    ui.horizontal(|ui| {
                        if ui.checkbox(&mut on, "").changed() {
                            if on {
                                st.picked.push(tid.clone());
                            } else {
                                st.picked.retain(|p| *p != tid);
                            }
                        }
                        let fields: Vec<String> = item["fields"]
                            .as_object()
                            .map(|m| {
                                m.iter()
                                    .map(|(k, v)| {
                                        format!("{k}: {} ({})", v["value"].as_str().unwrap_or_default(), v["source"].as_str().unwrap_or_default())
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        let issues: Vec<&str> = item["issues"].as_array().into_iter().flatten().filter_map(Value::as_str).map(issue_label).collect();
                        let head = format!("{} {}", item["kind"].as_str().unwrap_or_default(), item["level"].as_str().unwrap_or_default());
                        let text = if issues.is_empty() {
                            RichText::new(head)
                        } else {
                            RichText::new(format!("{head}  ! {}", issues.join(", "))).color(egui::Color32::from_rgb(220, 160, 40))
                        };
                        ui.label(text).on_hover_text(fields.join("\n"));
                    });
                }
            });
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                ui.label(crate::tl!("Field"));
                ui.add(egui::TextEdit::singleline(&mut st.field).desired_width(90.0).hint_text("type"));
                ui.label(crate::tl!("Value"));
                ui.add(egui::TextEdit::singleline(&mut st.value).desired_width(120.0));
            });
            ui.horizontal(|ui| {
                if ui.button(crate::tl!("Read from prints")).clicked() {
                    act = Some(("tags.read", json!({ "apply": true })));
                }
                let some = !st.picked.is_empty();
                if ui.add_enabled(some && !st.field.trim().is_empty(), egui::Button::new(crate::tl!("Set on Picked"))).clicked() {
                    act = Some(("tags.set", json!({ "ids": st.picked, "field": st.field.trim(), "value": st.value })));
                }
                if ui.add_enabled(some, egui::Button::new(crate::tl!("Accept Picked"))).clicked() {
                    act = Some(("tags.accept", json!({ "ids": st.picked })));
                }
                if ui.button(crate::tl!("Pick All Shown")).clicked() {
                    st.picked = list.iter().filter_map(|i| i["id"].as_str().map(String::from)).collect();
                }
            });
            if !st.report.is_empty() {
                ui.label(RichText::new(&st.report).small().color(t.text_dim));
            }
        });
    if let Some((cmd, p)) = act {
        st.report = match app.run(cmd, p) {
            Ok(v) => v.to_string(),
            Err(e) => e.to_string(),
        };
        st.picked.clear();
    }
    ctx.data_mut(|d| d.insert_temp(id, st));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(app: &mut CadApp) {
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| crate::dialogs::show(app, ui.ctx()));
        out.textures_delta.clear();
    }

    #[test]
    fn tags_opens_the_palette_and_renders() {
        let mut app = CadApp::new(cadaddy_engine::Session::new(), crate::Services::default());
        let tags = json!([{"id": "device:1:1.0,1.0", "kind": "luminaire", "level": "1", "at_ft": [1, 1], "issues": ["untagged"]}]);
        app.run("bspec.set", json!({ "spec": { "levels": [{"name": "1", "elevation_ft": 0, "height_ft": 12}], "tags": tags } })).unwrap();
        app.cmdline("elementtags");
        assert_eq!(app.ui.dialog.as_deref(), Some(DIALOG));
        frame(&mut app);
        let _ = crate::menus::run_ui_command(&mut app, "ui.language", &json!({ "lang": "ar" }));
        frame(&mut app);
        let mut empty = CadApp::new(cadaddy_engine::Session::new(), crate::Services::default());
        empty.ui.dialog = Some(DIALOG.into());
        frame(&mut empty);
    }
}
