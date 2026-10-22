//! The Project Rules palette (Tools ▸ Project Rules ▸ List, or PROJECTRULES at the command line):
//! lists all project rules grouped by domain (mounting heights, wiring methods, clearances, …),
//! each showing the active value, its source tier and citation, and any overridden values. Rules
//! are editable in-place; changes are previewed before applying and are undoable.

use egui::RichText;
use serde_json::{Value, json};

use crate::CadApp;

pub const DIALOG: &str = "projrules";

#[derive(Clone, Default)]
struct State {
    /// Filter by domain (None = all).
    domain_filter: Option<String>,
    /// Edit state: which rule is being edited (by domain/key id).
    editing: Option<EditState>,
}

#[derive(Clone, Default)]
struct EditState {
    domain: String,
    key: String,
    value: String,
    tier: String,
    source: String,
    reason: String,
}

fn id() -> egui::Id {
    egui::Id::new("projrules_palette_state")
}

fn cache_id() -> egui::Id {
    egui::Id::new("projrules_cache")
}

fn list_rules(app: &mut CadApp, domain: Option<&str>) -> Value {
    let mut params = json!({});
    if let Some(d) = domain {
        params["domain"] = json!(d);
    }
    app.session.execute("project.rules.list", &params).unwrap_or(Value::Null)
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let mut st: State = ctx.data(|d| d.get_temp(id())).unwrap_or_default();

    // Cache key: document pointer + filter.
    let doc_key = app.session.doc().ok().map(|d| d as *const _ as usize).unwrap_or(0);
    let key = (doc_key, st.domain_filter.clone());

    // Fetch or use cache.
    let data = match ctx.data_mut(|d| d.get_temp::<((usize, Option<String>), Value)>(cache_id())) {
        Some((k, v)) if k == key => v,
        _ => {
            let v = list_rules(app, st.domain_filter.as_deref());
            ctx.data_mut(|d| d.insert_temp(cache_id(), (key.clone(), v.clone())));
            v
        }
    };

    let label = data["label"].as_str().unwrap_or_default();
    let rules = data["rules"].as_array();

    let title = if label.is_empty() { crate::tl!("Project Rules").to_string() } else { format!("{} — {}", crate::tl!("Project Rules"), label) };

    egui::Window::new(title).id(egui::Id::new("projrules_palette")).default_size([420.0, 500.0]).open(open).show(ctx, |ui| {
        // Domain filter bar.
        ui.horizontal(|ui| {
            ui.label(crate::tl!("Domain:"));
            let domains = [
                ("All", None),
                ("Heights", Some("mounting_height")),
                ("Wiring", Some("wiring_method")),
                ("Studs", Some("stud_layout")),
                ("Joists", Some("joist_layout")),
                ("Clearance", Some("clearance")),
                ("Tolerance", Some("tolerance")),
                ("Naming", Some("naming")),
                ("Custom", Some("custom")),
            ];
            for (label, filter) in &domains {
                let current = st.domain_filter.as_deref();
                let selected = current == *filter;
                if ui.selectable_label(selected, *label).clicked() {
                    st.domain_filter = filter.map(String::from);
                    ctx.data_mut(|d| d.remove::<((usize, Option<String>), Value)>(cache_id()));
                }
            }
        });

        ui.separator();

        // Rules table.
        egui::ScrollArea::vertical().show(ui, |ui| {
            if let Some(rules) = rules {
                if rules.is_empty() {
                    ui.label(crate::tl!("No project rules set."));
                }
                for rule in rules {
                    let domain = rule["domain"].as_str().unwrap_or_default();
                    let key = rule["key"].as_str().unwrap_or_default();
                    let value = rule["value"].as_str().unwrap_or_default();
                    let tier = rule["tier"].as_str().unwrap_or_default();
                    let source = rule["source"].as_str().unwrap_or_default();
                    let overridden = rule["overridden"].as_array();

                    let header = format!("{domain}/{key}");
                    egui::CollapsingHeader::new(RichText::new(&header).strong()).id_salt(&header).default_open(false).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(crate::tl!("Value:"));
                            ui.label(RichText::new(value).strong());
                        });
                        ui.horizontal(|ui| {
                            ui.label(crate::tl!("Tier:"));
                            ui.label(tier_label(tier));
                        });
                        if !source.is_empty() {
                            ui.horizontal(|ui| {
                                ui.label(crate::tl!("Source:"));
                                ui.label(source);
                            });
                        }
                        if let Some(overrides) = overridden
                            && !overrides.is_empty()
                        {
                            ui.separator();
                            ui.label(RichText::new(crate::tl!("Overridden values:")).italics());
                            for ov in overrides {
                                let ov_val = ov["value"].as_str().unwrap_or_default();
                                let ov_tier = ov["tier"].as_str().unwrap_or_default();
                                let ov_src = ov["source"].as_str().unwrap_or_default();
                                ui.horizontal(|ui| {
                                    ui.label(format!("  {} ({})", ov_val, tier_label(ov_tier)));
                                    if !ov_src.is_empty() {
                                        ui.label(format!("from {}", ov_src));
                                    }
                                });
                            }
                        }

                        // Edit button.
                        ui.horizontal(|ui| {
                            if ui.button(crate::tl!("Edit")).clicked() {
                                st.editing = Some(EditState {
                                    domain: domain.to_string(),
                                    key: key.to_string(),
                                    value: value.to_string(),
                                    tier: tier.to_string(),
                                    source: source.to_string(),
                                    reason: String::new(),
                                });
                            }
                            if ui.button(crate::tl!("Remove")).clicked() {
                                app.run("project.rules.remove", json!({"domain": domain, "key": key})).ok();
                                ctx.data_mut(|d| d.remove::<((usize, Option<String>), Value)>(cache_id()));
                            }
                        });
                    });
                }
            } else {
                ui.label(crate::tl!("No drawing open."));
            }
        });

        ui.separator();

        // Inline editor (when editing).
        if st.editing.is_some() {
            let mut edit = st.editing.take().unwrap_or_default();
            let mut apply = false;
            let mut cancel = false;

            ui.group(|ui| {
                ui.label(RichText::new(format!("Edit: {}/{}", edit.domain, edit.key)).strong());
                ui.horizontal(|ui| {
                    ui.label(crate::tl!("Value:"));
                    ui.text_edit_singleline(&mut edit.value);
                });
                ui.horizontal(|ui| {
                    ui.label(crate::tl!("Tier:"));
                    egui::ComboBox::from_id_salt("edit_tier").selected_text(tier_label(&edit.tier)).show_ui(ui, |ui| {
                        for t in &["field_verified", "rfi", "submittal", "drawing_revision", "spec", "project_rule", "built_in"] {
                            ui.selectable_value(&mut edit.tier, t.to_string(), tier_label(t));
                        }
                    });
                });
                ui.horizontal(|ui| {
                    ui.label(crate::tl!("Source:"));
                    ui.text_edit_singleline(&mut edit.source);
                });
                ui.horizontal(|ui| {
                    ui.label(crate::tl!("Reason:"));
                    ui.text_edit_singleline(&mut edit.reason);
                });

                ui.horizontal(|ui| {
                    if ui.button(crate::tl!("Apply")).clicked() {
                        apply = true;
                    }
                    if ui.button(crate::tl!("Cancel")).clicked() {
                        cancel = true;
                    }
                });
            });

            if apply {
                app.run(
                    "project.rules.set",
                    json!({
                        "domain": edit.domain,
                        "key": edit.key,
                        "value": edit.value,
                        "tier": edit.tier,
                        "source": edit.source,
                        "reason": edit.reason,
                    }),
                )
                .ok();
                ctx.data_mut(|d| d.remove::<((usize, Option<String>), Value)>(cache_id()));
            } else if !cancel {
                st.editing = Some(edit);
            }
        }
    });

    ctx.data_mut(|d| d.insert_temp(id(), st));
}

fn tier_label(tier: &str) -> &str {
    match tier {
        "field_verified" => "Field-verified",
        "rfi" => "RFI / ASI",
        "submittal" => "Submittal",
        "drawing_revision" => "Drawing revision",
        "spec" => "Specification",
        "project_rule" => "Project rule",
        "built_in" => "Built-in default",
        _ => tier,
    }
}
