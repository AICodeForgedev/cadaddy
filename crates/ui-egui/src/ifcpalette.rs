//! The IFC Model palette (Window ▸ IFC Model; opens with File ▸ Open IFC Model...): an IFC file
//! opened as a model. Its spatial tree (site, building, storeys, spaces, classes; a click selects),
//! the selected object's properties and quantities, filters that isolate, hide or select by class,
//! storey, system or property, the section box, measuring between two objects, Walk, and the BCF
//! issues raised on the model (new from the view, status, assignee, comments, import and export).
//! Everything goes through the engine's `ifc.*` and `bcf.*` commands.

use egui::{Color32, RichText};
use serde_json::{Value, json};

use crate::CadApp;

pub const DIALOG: &str = "ifcmodel";

/// What the palette keeps between frames. Caches are keyed on revision (and selection for
/// properties) so we don't re-query the engine every frame.
#[derive(Clone, Default)]
struct State {
    /// The tree, and the drawing revision it was read at.
    tree: Option<(u64, Value)>,
    /// Cached properties: (revision, selected_handle, properties).
    props_cache: Option<(u64, String, Value)>,
    /// Cached bcf.list: (revision, topics).
    bcf_list_cache: Option<(u64, Value)>,
    /// Cached bcf.get: (revision, topic_guid, topic_detail).
    bcf_get_cache: Option<(u64, String, Value)>,
    class: String,
    level: String,
    system: String,
    property: String,
    measured: String,
    topic: Option<String>,
    title: String,
    comment: String,
    assignee: String,
    reply: String,
    message: String,
}

fn status_colour(s: &str) -> Color32 {
    match s.to_ascii_lowercase().as_str() {
        "closed" | "resolved" => Color32::from_rgb(90, 150, 90),
        "in progress" | "active" => Color32::from_rgb(220, 160, 40),
        _ => Color32::from_rgb(210, 60, 50),
    }
}

fn status_label(s: &str) -> &str {
    match s {
        "Open" => crate::tl!("Open"),
        "In Progress" => crate::tl!("In Progress"),
        "Resolved" => crate::tl!("Resolved"),
        "Closed" => crate::tl!("Closed"),
        other => other,
    }
}

/// What a click in the palette asks for, run after the window is drawn.
enum Act {
    Run(&'static str, Value),
    Select(String),
    Walk,
    Open,
    Import,
    Export,
}

fn tree_node(ui: &mut egui::Ui, n: &Value, selected: &[String], acts: &mut Vec<Act>, depth: usize) {
    let name = n["name"].as_str().unwrap_or_default();
    let count = n["count"].as_u64().unwrap_or(0);
    let kind = n["kind"].as_str().unwrap_or_default();
    let label = match kind {
        "storey" => format!("{name}  ({count})"),
        "class" => format!("{name}  ×{count}"),
        _ => format!("{name}  ({count})"),
    };
    let id = ui.make_persistent_id(("ifc-tree", depth, kind, name));
    egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, depth < 2 && kind != "class")
        .show_header(ui, |ui| {
            if ui.selectable_label(false, label).on_hover_text(crate::tl!("Isolate")).double_clicked() {
                let key = match kind {
                    "storey" => "level",
                    "space" => "space",
                    "class" => "class",
                    _ => "",
                };
                if !key.is_empty() {
                    acts.push(Act::Run("ifc.filter", json!({ key: name, "action": "isolate" })));
                }
            }
        })
        .body(|ui| {
            for c in n["children"].as_array().into_iter().flatten() {
                tree_node(ui, c, selected, acts, depth + 1);
            }
            for e in n["elements"].as_array().into_iter().flatten() {
                let h = e["handle"].as_str().unwrap_or_default().to_string();
                let text = e["name"].as_str().filter(|t| !t.is_empty()).unwrap_or(e["guid"].as_str().unwrap_or_default()).to_string();
                if ui.selectable_label(selected.contains(&h), text).on_hover_text(e["guid"].as_str().unwrap_or_default()).clicked() {
                    acts.push(Act::Select(h));
                }
            }
            if count > n["elements"].as_array().map_or(0, |a| a.len() as u64) && kind == "class" {
                ui.label(RichText::new(crate::tl!("More objects: filter to find them")).small().weak());
            }
        });
}

fn grid(ui: &mut egui::Ui, id: &str, rows: &[(&str, String)]) {
    egui::Grid::new(id).num_columns(2).striped(true).show(ui, |ui| {
        for (k, v) in rows.iter().filter(|(_, v)| !v.is_empty()) {
            ui.label(RichText::new(*k).weak());
            ui.label(v);
            ui.end_row();
        }
    });
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let t = crate::theme::Tokens::get();
    let id = egui::Id::new("ifc-palette-state");
    let mut st: State = ctx.data(|d| d.get_temp(id)).unwrap_or_default();
    let revision = app.session.state().map(|s| s.revision).unwrap_or(0);
    if st.tree.as_ref().is_none_or(|(r, _)| *r != revision) {
        st.tree = Some((revision, app.session.execute("ifc.tree", &json!({ "limit": 100 })).unwrap_or(Value::Null)));
    }
    let tree = st.tree.as_ref().map(|(_, v)| v.clone()).unwrap_or(Value::Null);
    let selected: Vec<String> = app.session.selection().iter().map(|h| h.hex()).collect();
    // Cache ifc.properties keyed on (revision, selected handle).
    let sel_key = if selected.len() == 1 { selected[0].clone() } else { String::new() };
    let props = if selected.len() == 1 {
        if st.props_cache.as_ref().is_none_or(|(r, h, _)| *r != revision || *h != sel_key) {
            let v = app.session.execute("ifc.properties", &json!({})).ok();
            st.props_cache = v.as_ref().map(|val| (revision, sel_key.clone(), val.clone()));
            v
        } else {
            st.props_cache.as_ref().map(|(_, _, v)| v.clone())
        }
    } else {
        None
    };
    // Cache bcf.list keyed on revision.
    let topics = if st.bcf_list_cache.as_ref().is_none_or(|(r, _)| *r != revision) {
        let v = app.session.execute("bcf.list", &json!({})).unwrap_or(Value::Null);
        st.bcf_list_cache = Some((revision, v.clone()));
        v
    } else {
        st.bcf_list_cache.as_ref().map(|(_, v)| v.clone()).unwrap_or(Value::Null)
    };
    // Cache bcf.get keyed on (revision, topic guid).
    let topic_guid = st.topic.clone().unwrap_or_default();
    let topic = if !topic_guid.is_empty() {
        if st.bcf_get_cache.as_ref().is_none_or(|(r, g, _)| *r != revision || *g != topic_guid) {
            let v = app.session.execute("bcf.get", &json!({ "topic": topic_guid })).ok();
            st.bcf_get_cache = v.as_ref().map(|val| (revision, topic_guid.clone(), val.clone()));
            v
        } else {
            st.bcf_get_cache.as_ref().map(|(_, _, v)| v.clone())
        }
    } else {
        None
    };
    let section_on = app.session.state().is_ok_and(|s| s.ifc_view.section.is_some());
    let mut acts: Vec<Act> = Vec::new();
    egui::Window::new(crate::tl!("IFC Model"))
        .id(egui::Id::new("IFC Model"))
        .default_pos(crate::palette_pos(app))
        .open(open)
        .resizable(true)
        .default_size([400.0, 720.0])
        .show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui.button(crate::tl!("Open IFC Model...")).clicked() {
                    acts.push(Act::Open);
                }
                if ui.button(crate::tl!("Walk")).clicked() {
                    acts.push(Act::Walk);
                }
                if let Some(n) = tree["elements"].as_u64() {
                    ui.label(RichText::new(crate::tf!("{n} objects", n = n)).color(t.text_dim));
                }
            });
            if tree.is_null() {
                ui.label(RichText::new(crate::tl!("No IFC model is open: File ▸ Open IFC Model...")).color(t.text_dim));
            }
            egui::ScrollArea::vertical().id_salt("ifc-palette").show(ui, |ui| {
                if !tree.is_null() {
                    egui::CollapsingHeader::new(crate::tl!("Model tree")).default_open(true).show(ui, |ui| {
                        egui::ScrollArea::vertical().id_salt("ifc-tree").max_height(220.0).show(ui, |ui| {
                            for n in tree["tree"].as_array().into_iter().flatten() {
                                tree_node(ui, n, &selected, &mut acts, 0);
                            }
                        });
                    });
                    egui::CollapsingHeader::new(crate::tl!("Properties")).default_open(true).show(ui, |ui| match &props {
                        Some(p) => {
                            let s = |k: &str| p[k].as_str().unwrap_or_default().to_string();
                            let size = p["size"]
                                .as_array()
                                .map(|a| a.iter().filter_map(Value::as_f64).map(|v| format!("{v:.2}")).collect::<Vec<_>>().join(" × "))
                                .unwrap_or_default();
                            let systems = p["systems"]
                                .as_array()
                                .map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "))
                                .unwrap_or_default();
                            grid(
                                ui,
                                "ifc-props-id",
                                &[
                                    (crate::tl!("Class"), s("class")),
                                    (crate::tl!("Name"), s("name")),
                                    ("GlobalId", s("guid")),
                                    (crate::tl!("Type"), s("type")),
                                    (crate::tl!("Storey"), s("storey")),
                                    (crate::tl!("Space"), s("space")),
                                    (crate::tl!("Building"), s("building")),
                                    (crate::tl!("Systems"), systems),
                                    (crate::tl!("Size"), size),
                                ],
                            );
                            for key in ["property_sets", "quantity_sets"] {
                                for (set, vals) in p[key].as_object().into_iter().flatten() {
                                    egui::CollapsingHeader::new(set.as_str()).id_salt(("ifc-set", key, set.as_str())).default_open(true).show(
                                        ui,
                                        |ui| {
                                            let rows: Vec<(&str, String)> = vals
                                                .as_object()
                                                .into_iter()
                                                .flatten()
                                                .map(|(k, v)| (k.as_str(), v.as_str().unwrap_or_default().to_string()))
                                                .collect();
                                            grid(ui, &format!("ifc-set-{key}-{set}"), &rows);
                                        },
                                    );
                                }
                            }
                        }
                        None => {
                            ui.label(RichText::new(crate::tl!("Select one object to see its properties.")).color(t.text_dim));
                        }
                    });
                    egui::CollapsingHeader::new(crate::tl!("Filter")).show(ui, |ui| {
                        egui::Grid::new("ifc-filter").num_columns(2).show(ui, |ui| {
                            for (label, field, hint) in [
                                (crate::tl!("Class"), &mut st.class, "Wall"),
                                (crate::tl!("Storey"), &mut st.level, "Level 1"),
                                (crate::tl!("System"), &mut st.system, "Supply Air"),
                                (crate::tl!("Property"), &mut st.property, "Pset_WallCommon.FireRating=2HR"),
                            ] {
                                ui.label(label);
                                ui.add(egui::TextEdit::singleline(field).hint_text(hint).desired_width(220.0));
                                ui.end_row();
                            }
                        });
                        let mut q = json!({});
                        for (k, v) in [("class", &st.class), ("level", &st.level), ("system", &st.system), ("property", &st.property)] {
                            if !v.trim().is_empty() {
                                q[k] = json!(v.trim());
                            }
                        }
                        ui.horizontal(|ui| {
                            for (label, action) in
                                [(crate::tl!("Isolate"), "isolate"), (crate::tl!("Hide"), "hide"), (crate::tl!("Select"), "select")]
                            {
                                if ui.add_enabled(q.as_object().is_some_and(|o| !o.is_empty()), egui::Button::new(label)).clicked() {
                                    let mut q = q.clone();
                                    q["action"] = json!(action);
                                    acts.push(Act::Run("ifc.filter", q));
                                }
                            }
                            if ui.button(crate::tl!("Show All")).clicked() {
                                acts.push(Act::Run("ifc.filter", json!({ "action": "show" })));
                            }
                        });
                    });
                    egui::CollapsingHeader::new(crate::tl!("Section box and measure")).show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            if ui.add_enabled(!selected.is_empty(), egui::Button::new(crate::tl!("Box the selection"))).clicked() {
                                acts.push(Act::Run("ifc.sectionbox", json!({ "fit": "selection" })));
                            }
                            if ui.add_enabled(section_on, egui::Button::new(crate::tl!("Section box off"))).clicked() {
                                acts.push(Act::Run("ifc.sectionbox", json!({ "on": false })));
                            }
                            if ui.add_enabled(selected.len() == 2, egui::Button::new(crate::tl!("Measure the two selected"))).clicked() {
                                acts.push(Act::Run("ifc.measure", json!({})));
                            }
                        });
                        if !st.measured.is_empty() {
                            ui.label(&st.measured);
                        }
                    });
                }
                egui::CollapsingHeader::new(crate::tf!("BCF issues ({n})", n = topics["total"].as_u64().unwrap_or(0))).default_open(true).show(
                    ui,
                    |ui| {
                        for tp in topics["topics"].as_array().into_iter().flatten() {
                            let g = tp["guid"].as_str().unwrap_or_default().to_string();
                            let status = tp["status"].as_str().unwrap_or_default();
                            ui.horizontal(|ui| {
                                // The status as a dot (painted: not every interface font has a circle).
                                let (r, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                                ui.painter().circle_filled(r.center(), 4.0, status_colour(status));
                                let text = format!("#{}  {}", tp["index"], tp["title"].as_str().unwrap_or_default());
                                let who = tp["assignee"].as_str().unwrap_or_default();
                                if ui
                                    .selectable_label(st.topic.as_deref() == Some(g.as_str()), text)
                                    .on_hover_text(format!("{} — {who}", status_label(status)))
                                    .clicked()
                                {
                                    st.topic = Some(g.clone());
                                    acts.push(Act::Run("bcf.show", json!({ "topic": g })));
                                }
                            });
                        }
                        if let Some(tp) = &topic {
                            let g = tp["guid"].as_str().unwrap_or_default().to_string();
                            ui.separator();
                            ui.label(RichText::new(tp["title"].as_str().unwrap_or_default()).strong());
                            if let Some(d) = tp["description"].as_str().filter(|d| !d.is_empty()) {
                                ui.label(d);
                            }
                            ui.horizontal(|ui| {
                                let status = tp["status"].as_str().unwrap_or_default().to_string();
                                egui::ComboBox::from_id_salt("bcf-status").selected_text(status_label(&status)).show_ui(ui, |ui| {
                                    for s in cadaddy_engine::cmd::bcf::STATUSES {
                                        if ui.selectable_label(status == s, status_label(s)).clicked() && status != s {
                                            acts.push(Act::Run("bcf.update", json!({ "topic": g, "status": s })));
                                        }
                                    }
                                });
                                ui.label(RichText::new(tp["assignee"].as_str().unwrap_or_default()).color(t.text_dim));
                            });
                            for c in tp["comments"].as_array().into_iter().flatten() {
                                ui.label(
                                    RichText::new(format!(
                                        "{} · {}",
                                        c["author"].as_str().unwrap_or_default(),
                                        c["date"].as_str().unwrap_or_default().get(..10).unwrap_or_default()
                                    ))
                                    .small()
                                    .color(t.text_dim),
                                );
                                ui.label(c["text"].as_str().unwrap_or_default());
                            }
                            ui.horizontal(|ui| {
                                ui.add(egui::TextEdit::singleline(&mut st.reply).hint_text(crate::tl!("Add a comment")).desired_width(220.0));
                                if ui.add_enabled(!st.reply.trim().is_empty(), egui::Button::new(crate::tl!("Add"))).clicked() {
                                    acts.push(Act::Run("bcf.update", json!({ "topic": g, "comment": st.reply.trim() })));
                                    st.reply.clear();
                                }
                            });
                            ui.horizontal(|ui| {
                                if ui.button(crate::tl!("Show")).clicked() {
                                    acts.push(Act::Run("bcf.show", json!({ "topic": g })));
                                }
                                if ui.button(crate::tl!("Delete")).clicked() {
                                    acts.push(Act::Run("bcf.delete", json!({ "topic": g })));
                                    st.topic = None;
                                }
                            });
                        }
                        ui.separator();
                        ui.label(RichText::new(crate::tl!("New issue from this view")).strong());
                        egui::Grid::new("bcf-new").num_columns(2).show(ui, |ui| {
                            ui.label(crate::tl!("Title"));
                            ui.add(egui::TextEdit::singleline(&mut st.title).desired_width(220.0));
                            ui.end_row();
                            ui.label(crate::tl!("Comment"));
                            ui.add(egui::TextEdit::multiline(&mut st.comment).desired_rows(2).desired_width(220.0));
                            ui.end_row();
                            ui.label(crate::tl!("Assign to"));
                            ui.add(egui::TextEdit::singleline(&mut st.assignee).desired_width(220.0));
                            ui.end_row();
                        });
                        ui.horizontal(|ui| {
                            if ui.add_enabled(!st.title.trim().is_empty(), egui::Button::new(crate::tl!("Create Issue"))).clicked() {
                                acts.push(Act::Run(
                                    "bcf.create",
                                    json!({ "title": st.title.trim(), "comment": st.comment.trim(), "assignee": st.assignee.trim() }),
                                ));
                                st.title.clear();
                                st.comment.clear();
                            }
                            if ui.button(crate::tl!("Import BCF...")).clicked() {
                                acts.push(Act::Import);
                            }
                            if ui.add_enabled(topics["total"].as_u64().unwrap_or(0) > 0, egui::Button::new(crate::tl!("Export BCF..."))).clicked() {
                                acts.push(Act::Export);
                            }
                        });
                    },
                );
                if !st.message.is_empty() {
                    ui.label(RichText::new(&st.message).small().color(t.text_dim));
                }
            });
        });
    for a in acts {
        let r = match a {
            Act::Run(cmd, p) => app.run(cmd, p).map(|v| (cmd, v)),
            Act::Select(h) => {
                if let Some(h) = cadaddy_engine::doc::Handle::parse_hex(&h) {
                    app.session.set_selection(vec![h]);
                }
                continue;
            }
            Act::Walk => {
                if let Err(e) = app.run("ui.walk", Value::Null) {
                    st.message = e;
                }
                continue;
            }
            Act::Open => app.run("ifc.open", Value::Null).map(|v| ("ifc.open", v)),
            Act::Import => app.run("bcf.import", Value::Null).map(|v| ("bcf.import", v)),
            Act::Export => app.run("bcf.export", Value::Null).map(|v| ("bcf.export", v)),
        };
        match r {
            Ok(("ifc.measure", v)) => {
                st.measured = format!(
                    "{} {:.3}  ({} {:.3})",
                    crate::tl!("Between centres:"),
                    v["distance"].as_f64().unwrap_or(0.0),
                    crate::tl!("gap"),
                    v["gap"].as_f64().unwrap_or(0.0)
                );
            }
            Ok(("bcf.create", v)) => {
                st.topic = v["guid"].as_str().map(String::from);
                st.message = crate::tf!("Issue #{n} created", n = v["index"]);
            }
            Ok(("bcf.import", v)) if !v.is_null() => {
                st.message = crate::tf!("BCF: {added} added, {updated} updated", added = v["added"], updated = v["updated"]);
            }
            Ok(("bcf.show", v)) if v["missing"].as_array().is_some_and(|m| !m.is_empty()) => {
                st.message = crate::tf!("{n} of its objects aren't in this model", n = v["missing"].as_array().map_or(0, Vec::len));
            }
            Ok(_) => {}
            Err(e) => st.message = e,
        }
    }
    ctx.data_mut(|d| d.insert_temp(id, st));
}

#[cfg(test)]
mod tests {
    use super::*;

    const IFC: &str = "ISO-10303-21;\nHEADER;FILE_DESCRIPTION((''),'2;1');FILE_NAME('m.ifc','',(''),(''),'','','');FILE_SCHEMA(('IFC4'));ENDSEC;\nDATA;\n\
        #1=IFCSIUNIT(*,.LENGTHUNIT.,$,.METRE.);\n#2=IFCUNITASSIGNMENT((#1));\n#3=IFCPROJECT('0Prj000000000000000001',$,'P',$,$,$,$,$,#2);\n\
        #4=IFCSITE('0Sit000000000000000001',$,'Site',$,$,$,$,$,.ELEMENT.,$,$,$,$,$);\n#5=IFCBUILDING('0Bld000000000000000001',$,'Block A',$,$,$,$,$,.ELEMENT.,$,$,$);\n\
        #6=IFCBUILDINGSTOREY('0Sty000000000000000001',$,'Level 1',$,$,$,$,$,.ELEMENT.,0.);\n\
        #7=IFCRELAGGREGATES('0Rel000000000000000001',$,$,$,#3,(#4));\n#8=IFCRELAGGREGATES('0Rel000000000000000002',$,$,$,#4,(#5));\n#9=IFCRELAGGREGATES('0Rel000000000000000003',$,$,$,#5,(#6));\n\
        #10=IFCCARTESIANPOINT((0.,0.,0.));\n#11=IFCAXIS2PLACEMENT3D(#10,$,$);\n#12=IFCLOCALPLACEMENT($,#11);\n#13=IFCCARTESIANPOINT((0.,0.));\n#14=IFCAXIS2PLACEMENT2D(#13,$);\n\
        #15=IFCRECTANGLEPROFILEDEF(.AREA.,$,#14,6.,0.2);\n#16=IFCDIRECTION((0.,0.,1.));\n#17=IFCEXTRUDEDAREASOLID(#15,$,#16,3.);\n#18=IFCSHAPEREPRESENTATION($,'Body','SweptSolid',(#17));\n\
        #19=IFCPRODUCTDEFINITIONSHAPE($,$,(#18));\n#20=IFCWALL('1Wal000000000000000001',$,'Wall A',$,$,#12,#19,$,.STANDARD.);\n\
        #21=IFCPROPERTYSINGLEVALUE('FireRating',$,IFCLABEL('1HR'),$);\n#22=IFCPROPERTYSET('0Pst000000000000000001',$,'Pset_WallCommon',$,(#21));\n\
        #23=IFCRELDEFINESBYPROPERTIES('0Rel000000000000000004',$,$,$,(#20),#22);\n#24=IFCRELCONTAINEDINSPATIALSTRUCTURE('0Rel000000000000000005',$,$,$,(#20),#6);\nENDSEC;\nEND-ISO-10303-21;\n";

    fn frame(app: &mut CadApp) {
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| crate::dialogs::show(app, ui.ctx()));
        out.textures_delta.clear();
    }

    #[test]
    fn the_palette_opens_on_a_model_and_renders_its_tree_properties_and_issues() {
        let dir = std::env::temp_dir().join(format!("cadaddy-ifcpalette-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("m.ifc");
        std::fs::write(&path, IFC).unwrap();
        let mut app = CadApp::new(cadaddy_engine::Session::new(), crate::Services::default());
        // Empty: the palette says how to open a model.
        app.cmdline("ifcmodel");
        assert_eq!(app.ui.dialog.as_deref(), Some(DIALOG));
        frame(&mut app);
        // The model read as ifc.open reads it (the file hooks are the test process's, so not through them).
        let d = cadaddy_io::read(std::fs::read(&path).unwrap().as_slice(), "m.ifc").unwrap();
        app.session.open_drawing(d, "m", None);
        let wall = app.run("ifc.properties", json!({ "guid": "1Wal000000000000000001" })).unwrap();
        app.session.set_selection(vec![cadaddy_engine::doc::Handle::parse_hex(wall["handle"].as_str().unwrap()).unwrap()]);
        app.run("bcf.create", json!({ "title": "Rating?", "comment": "1HR here?" })).unwrap();
        app.ui.dialog = Some(DIALOG.into());
        frame(&mut app);
        frame(&mut app);
        // Walk opens on the model (its storeys as levels).
        app.run("ui.walk", Value::Null).unwrap();
        assert!(app.walk.as_ref().is_some_and(|w| w.building.scene.levels.len() == 1));
        let _ = crate::menus::run_ui_command(&mut app, "ui.language", &json!({ "lang": "uk" }));
        app.ui.dialog = Some(DIALOG.into());
        frame(&mut app);
    }
}
