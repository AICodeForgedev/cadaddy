//! Tool palettes (TOOLPALETTES): the user's own palettes of tools — blocks, hatches, commands and
//! linetypes — made by dropping objects from the drawing onto a palette (or Add Selected): a
//! block reference becomes its block, a hatch its pattern, scale and angle, anything else the
//! command that draws it with its layer and linetype. Using a tool starts its command with those
//! settings. Palettes are kept with the interface state and export to and import from a JSON file
//! of CADaddy's own format (`{"cadaddyToolPalettes": 1, "palettes": [...]}`).

use egui::RichText;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::CadApp;
use crate::blockspalette::BlockRef;

/// Most palettes, and tools in one.
pub const MAX_PALETTES: usize = 50;
pub const MAX_TOOLS: usize = 500;
/// Largest palette file read.
const MAX_FILE_BYTES: u64 = 8 << 20;

/// One tool.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Tool {
    Block {
        name: String,
        file: Option<String>,
    },
    Hatch {
        pattern: String,
        scale: f64,
        angle: f64,
    },
    /// A command, run with the layer and linetype it was made with current.
    Command {
        command: String,
        layer: Option<String>,
        linetype: Option<String>,
    },
    Linetype {
        name: String,
    },
}

impl Tool {
    pub fn label(&self) -> String {
        match self {
            Tool::Block { name, .. } => name.clone(),
            Tool::Hatch { pattern, .. } => pattern.clone(),
            Tool::Command { command, .. } => command.to_ascii_uppercase(),
            Tool::Linetype { name } => name.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Palette {
    pub name: String,
    pub tools: Vec<Tool>,
}

/// The palettes a new profile starts with: one empty palette to drop tools on.
pub fn default_palettes() -> Vec<Palette> {
    vec![Palette { name: "My Tools".into(), tools: Vec::new() }]
}

/// The tool an object of the drawing makes.
pub fn tool_for(e: &cadaddy_engine::doc::Entity) -> Option<Tool> {
    use cadaddy_engine::doc::EntityKind as K;
    let layer = Some(e.common.layer.clone()).filter(|l| !l.is_empty());
    let linetype = Some(e.common.linetype.clone()).filter(|l| !l.is_empty() && !l.eq_ignore_ascii_case("bylayer"));
    let command = |c: &str| Some(Tool::Command { command: c.into(), layer: layer.clone(), linetype: linetype.clone() });
    match &e.kind {
        K::Insert(i) => Some(Tool::Block { name: i.block.clone(), file: None }),
        K::Hatch(h) => {
            Some(Tool::Hatch { pattern: if h.solid { "SOLID".into() } else { h.pattern.clone() }, scale: h.scale, angle: h.angle.to_degrees() })
        }
        K::Line(_) => command("line"),
        K::Circle(_) => command("circle"),
        K::Arc(_) => command("arc"),
        K::LwPolyline(_) => command("pline"),
        K::Ellipse(_) => command("ellipse"),
        K::Spline(_) => command("spline"),
        K::Text(_) => command("text"),
        K::MText(_) => command("mtext"),
        K::Dimension(_) => command("dimlinear"),
        K::Point(_) => command("point"),
        _ => None,
    }
}

fn palette_mut<'a>(app: &'a mut CadApp, name: &str) -> Result<&'a mut Palette, String> {
    app.ui.tool_palettes.iter_mut().find(|p| p.name.eq_ignore_ascii_case(name)).ok_or_else(|| format!("no tool palette \"{name}\""))
}

/// Add `tools` to palette `name` (made when missing); a tool it already has isn't added twice.
pub fn add_tools(app: &mut CadApp, name: &str, tools: Vec<Tool>) -> Result<usize, String> {
    if !app.ui.tool_palettes.iter().any(|p| p.name.eq_ignore_ascii_case(name)) {
        if app.ui.tool_palettes.len() >= MAX_PALETTES {
            return Err(format!("at most {MAX_PALETTES} tool palettes"));
        }
        app.ui.tool_palettes.push(Palette { name: name.trim().to_string(), tools: Vec::new() });
    }
    let p = palette_mut(app, name)?;
    let mut n = 0;
    for t in tools {
        if p.tools.len() >= MAX_TOOLS {
            break;
        }
        if !p.tools.contains(&t) {
            p.tools.push(t);
            n += 1;
        }
    }
    Ok(n)
}

/// The tools the selected objects make.
pub fn selected_tools(app: &CadApp) -> Vec<Tool> {
    let Ok(st) = app.session.state() else { return Vec::new() };
    let Ok(d) = app.session.doc() else { return Vec::new() };
    st.selection.iter().filter_map(|h| d.entity(*h)).filter_map(|e| tool_for(e)).collect()
}

/// Use a tool: insert the block, hatch with the pattern, run the command with its layer and
/// linetype current, or make the linetype current.
pub fn use_tool(app: &mut CadApp, t: &Tool) {
    let set = |app: &mut CadApp, var: &str, v: Value| {
        if let Err(e) = cadaddy_engine::sysvars::set(&mut app.session, var, &v) {
            app.session.echo(e.user_message());
        }
    };
    match t {
        Tool::Block { name, file } => crate::blockspalette::start_insert(app, &BlockRef { name: name.clone(), file: file.clone() }),
        Tool::Hatch { pattern, scale, angle } => {
            set(app, "HPNAME", json!(pattern));
            set(app, "HPSCALE", json!(scale));
            set(app, "HPANG", json!(angle));
            app.start("hatch");
        }
        Tool::Command { command, layer, linetype } => {
            if let Some(l) = layer.as_ref().filter(|l| app.session.doc().is_ok_and(|d| d.layer(l).is_some())) {
                set(app, "CLAYER", json!(l));
            }
            if let Some(l) = linetype.as_ref().filter(|l| app.session.doc().is_ok_and(|d| d.linetype(l).is_some())) {
                set(app, "CELTYPE", json!(l));
            }
            app.start(command);
        }
        Tool::Linetype { name } => set(app, "CELTYPE", json!(name)),
    }
}

/// The palettes as the export file holds them.
pub fn export_json(app: &CadApp) -> String {
    serde_json::to_string_pretty(&json!({ "cadaddyToolPalettes": 1, "palettes": app.ui.tool_palettes })).unwrap_or_default()
}

/// Read palettes from an export file's text: they are added, replacing palettes of the same name.
pub fn import_json(app: &mut CadApp, text: &str) -> Result<usize, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("not a tool palette file: {e}"))?;
    if v.get("cadaddyToolPalettes").and_then(Value::as_u64) != Some(1) {
        return Err("not a CADaddy tool palette file".into());
    }
    let mut palettes: Vec<Palette> = serde_json::from_value(v["palettes"].clone()).map_err(|e| format!("not a tool palette file: {e}"))?;
    palettes.truncate(MAX_PALETTES);
    let n = palettes.len();
    for mut p in palettes {
        p.tools.truncate(MAX_TOOLS);
        app.ui.tool_palettes.retain(|o| !o.name.eq_ignore_ascii_case(&p.name));
        if app.ui.tool_palettes.len() < MAX_PALETTES {
            app.ui.tool_palettes.push(p);
        }
    }
    Ok(n)
}

/// TOOLPALETTES {action?: list (default) | add (palette, tools? | selected: true) | remove
/// (palette, index?) | export (path) | import (path)}.
pub fn run(app: &mut CadApp, p: &Value) -> Result<Value, String> {
    let s = |k: &str| p.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
    match s("action").to_ascii_lowercase().as_str() {
        "" | "list" => {}
        "add" => {
            let name = Some(s("palette")).filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "My Tools".into());
            let tools: Vec<Tool> = if p.get("selected").and_then(Value::as_bool) == Some(true) {
                selected_tools(app)
            } else {
                serde_json::from_value(p.get("tools").cloned().unwrap_or(Value::Null)).map_err(|e| format!("`tools`: {e}"))?
            };
            let n = add_tools(app, &name, tools)?;
            return Ok(json!({ "added": n, "palettes": app.ui.tool_palettes }));
        }
        "remove" => {
            let name = s("palette");
            match p.get("index").and_then(Value::as_u64) {
                Some(i) => {
                    let pal = palette_mut(app, &name)?;
                    let i = usize::try_from(i).map_err(|_| "bad index".to_string())?;
                    if i >= pal.tools.len() {
                        return Err(format!("palette \"{name}\" has {} tools", pal.tools.len()));
                    }
                    pal.tools.remove(i);
                }
                None => {
                    let n = app.ui.tool_palettes.len();
                    app.ui.tool_palettes.retain(|o| !o.name.eq_ignore_ascii_case(&name));
                    if app.ui.tool_palettes.len() == n {
                        return Err(format!("no tool palette \"{name}\""));
                    }
                }
            }
        }
        "export" => {
            let path = s("path");
            std::fs::write(&path, export_json(app)).map_err(|e| format!("{path}: {e}"))?;
        }
        "import" => {
            let path = s("path");
            let size = std::fs::metadata(&path).map_err(|e| format!("{path}: {e}"))?.len();
            if size > MAX_FILE_BYTES {
                return Err(format!("{path}: too large"));
            }
            let text = std::fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
            import_json(app, &text)?;
        }
        other => return Err(format!("unknown action \"{other}\" (list, add, remove, export or import)")),
    }
    Ok(json!({ "palettes": app.ui.tool_palettes }))
}

#[derive(Clone, Default)]
struct State {
    tab: usize,
    new_name: String,
    error: Option<String>,
}

fn id() -> egui::Id {
    egui::Id::new("cadaddy-tool-palettes")
}

/// The Tool Palettes window: a tab per palette, its tools as buttons; objects dropped on it from
/// the drawing (a drag that starts on the selection) become tools.
pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let t = crate::theme::Tokens::get();
    let mut st: State = ctx.data(|d| d.get_temp(id())).unwrap_or_default();
    if app.ui.tool_palettes.is_empty() {
        app.ui.tool_palettes = default_palettes();
    }
    st.tab = st.tab.min(app.ui.tool_palettes.len().saturating_sub(1));
    let mut use_it: Option<Tool> = None;
    let mut remove: Option<usize> = None;
    let mut add_selected = false;
    let (mut export, mut import) = (false, false);
    let window = egui::Window::new(crate::tl!("Tool Palettes"))
        .id(egui::Id::new("Tool Palettes"))
        .default_pos(crate::palette_pos(app))
        .open(open)
        .resizable(true)
        .default_size([260.0, 420.0])
        .show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                for (i, p) in app.ui.tool_palettes.iter().enumerate() {
                    if ui.selectable_label(st.tab == i, &p.name).clicked() {
                        st.tab = i;
                    }
                }
            });
            ui.separator();
            if let Some(p) = app.ui.tool_palettes.get(st.tab) {
                egui::ScrollArea::vertical().id_salt("tool-list").max_height(280.0).show(ui, |ui| {
                    if p.tools.is_empty() {
                        ui.label(RichText::new(crate::tl!("Drag objects from the drawing here to make tools.")).color(t.text_dim));
                    }
                    for (i, tool) in p.tools.iter().enumerate() {
                        let kind = match tool {
                            Tool::Block { .. } => "Block",
                            Tool::Hatch { .. } => "Hatch",
                            Tool::Command { .. } => "Command",
                            Tool::Linetype { .. } => "Linetype",
                        };
                        let r = ui.add(
                            egui::Button::new(format!("{}  ·  {}", tool.label(), crate::i18n::t(kind)))
                                .min_size(egui::vec2(ui.available_width(), 22.0)),
                        );
                        if r.clicked() {
                            use_it = Some(tool.clone());
                        }
                        r.context_menu(|ui| {
                            if ui.button(crate::tl!("Delete")).clicked() {
                                remove = Some(i);
                                ui.close();
                            }
                        });
                    }
                });
            }
            ui.separator();
            add_selected = ui.button(crate::tl!("Add Selected")).clicked();
            ui.horizontal(|ui| {
                let r = ui.add(egui::TextEdit::singleline(&mut st.new_name).desired_width(120.0).hint_text(crate::tl!("New palette")));
                crate::icons::describe_area(&r, "New palette");
                if ui.add_enabled(!st.new_name.trim().is_empty(), egui::Button::new(crate::tl!("Add"))).clicked() {
                    let name = st.new_name.trim().to_string();
                    st.error = add_tools(app, &name, Vec::new()).err();
                    st.new_name.clear();
                }
            });
            ui.horizontal(|ui| {
                export = ui.button(crate::tl!("Export...")).clicked();
                import = ui.button(crate::tl!("Import...")).clicked();
            });
            if let Some(e) = &st.error {
                ui.label(RichText::new(e).color(t.warn));
            }
        });
    let name = app.ui.tool_palettes.get(st.tab).map(|p| p.name.clone()).unwrap_or_else(|| "My Tools".into());
    // The selection dragged from the drawing area and dropped on the palette.
    let dropped = window.as_ref().is_some_and(|w| {
        ctx.input(|i| {
            i.pointer.any_released()
                && i.pointer.press_origin().is_some_and(|o| app.canvas.rect.is_some_and(|c| c.contains(o)) && !w.response.rect.contains(o))
                && i.pointer.hover_pos().is_some_and(|p| w.response.rect.contains(p))
        })
    });
    if add_selected || dropped {
        let tools = selected_tools(app);
        st.error = match add_tools(app, &name, tools) {
            Ok(0) if add_selected => Some(crate::tl!("Select objects in the drawing first.").to_string()),
            Ok(_) => None,
            Err(e) => Some(e),
        };
    }
    if let Some(i) = remove {
        let _ = run(app, &json!({ "action": "remove", "palette": name, "index": i }));
    }
    if let Some(tool) = use_it {
        use_tool(app, &tool);
    }
    if export && let Some(Some(path)) = app.services.pick_save.as_ref().map(|p| p("tool palettes.json")) {
        st.error = run(app, &json!({ "action": "export", "path": path })).err();
    }
    if import && let Some(Some(path)) = app.services.pick_open.as_ref().map(|p| p()) {
        st.error = run(app, &json!({ "action": "import", "path": path })).err();
    }
    if *open {
        ctx.data_mut(|d| d.insert_temp(id(), st));
    } else {
        ctx.data_mut(|d| d.remove::<State>(id()));
    }
}
