//! Hyperlinks in the app: the Insert Hyperlink dialog (typed HYPERLINK, Insert ▸ Hyperlink…, ⌘K /
//! Ctrl+K), the tooltip over a linked object and Ctrl+click (⌘-click on macOS) to follow its
//! link. The engine keeps and resolves the links (`hyperlink`, `hyperlink.open`); a URL opens
//! through the host (egui's open-URL output: the browser or the system's file opener).

use cadaddy_engine::doc::Handle;
use cadaddy_engine::doc::hyperlink::{self, Hyperlink};
use egui::vec2;
use serde_json::json;

use crate::CadApp;

/// HYPERLINK typed or from the menu: the dialog for the selected objects; with none selected,
/// select them first ([`resume`] opens the dialog after).
pub fn start(app: &mut CadApp) {
    if app.session.selection().is_empty() {
        app.hyperlink_pick = true;
        app.start("hyperlink.pick");
    } else {
        app.ui.dialog = Some(DIALOG.into());
    }
}

/// Every frame: once the selection HYPERLINK started has ended, open the dialog for it.
pub fn resume(app: &mut CadApp) {
    if !app.hyperlink_pick || app.session.running.is_some() {
        return;
    }
    app.hyperlink_pick = false;
    if !app.session.selection().is_empty() {
        app.ui.dialog = Some(DIALOG.into());
    }
}

/// The dialog's name in `ui.dialog`.
pub const DIALOG: &str = "hyperlink";

/// What the dialog edits.
#[derive(Clone, Debug, Default)]
struct State {
    handles: Vec<Handle>,
    link: Hyperlink,
    /// Some of the objects have a link (Remove Link is offered).
    linked: bool,
}

fn state_for(app: &CadApp) -> State {
    let handles = app.session.selection();
    let Ok(d) = app.session.doc() else { return State::default() };
    let links: Vec<Hyperlink> = handles.iter().filter_map(|h| hyperlink::read(&d.entity(*h)?.common.xdata)).collect();
    State { link: links.first().cloned().unwrap_or_default(), linked: !links.is_empty(), handles }
}

/// The named locations of this drawing: Model, its layouts, its named views.
fn locations(app: &CadApp) -> Vec<String> {
    let Ok(d) = app.session.doc() else { return Vec::new() };
    std::iter::once("Model".to_string()).chain(d.layouts.iter().map(|l| l.name.clone())).chain(d.views.iter().map(|v| v.name.clone())).collect()
}

/// The Insert Hyperlink dialog.
pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let id = egui::Id::new("cc_hyperlink_dialog");
    let mut st = ctx.data(|d| d.get_temp::<State>(id)).unwrap_or_else(|| state_for(app));
    let places = locations(app);
    let (mut ok, mut cancel, mut remove) = (false, false, false);
    let title = if st.linked { crate::tl!("Edit Hyperlink") } else { crate::tl!("Insert Hyperlink") };
    let center = ctx.content_rect().center();
    egui::Window::new(title)
        .id(id.with("window"))
        .open(open)
        .resizable(false)
        .collapsible(false)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(center)
        .show(ctx, |ui| {
            egui::Grid::new("cc_hyperlink_grid").num_columns(2).spacing(vec2(12.0, 8.0)).show(ui, |ui| {
                ui.label(crate::tl!("Text to display"));
                ui.add(egui::TextEdit::singleline(&mut st.link.description).desired_width(360.0));
                ui.end_row();
                ui.label(crate::tl!("File or web page"));
                ui.add(egui::TextEdit::singleline(&mut st.link.url).desired_width(360.0).hint_text("https://…"));
                ui.end_row();
                ui.label(crate::tl!("Named location"));
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut st.link.target).desired_width(250.0));
                    egui::ComboBox::from_id_salt("cc_hyperlink_places").selected_text(crate::tl!("This drawing")).width(100.0).show_ui(ui, |ui| {
                        for p in &places {
                            if ui.selectable_label(st.link.target == *p, p).clicked() {
                                st.link.target = p.clone();
                                st.link.url.clear();
                            }
                        }
                    });
                });
                ui.end_row();
            });
            ui.label(
                egui::RichText::new(crate::tl!("A named location with no file or web page is a layout or view of this drawing."))
                    .small()
                    .color(crate::theme::Tokens::get().text_dim),
            );
            ui.separator();
            ui.horizontal(|ui| {
                ok = ui.add_enabled(!st.link.is_empty(), egui::Button::new(crate::tl!("OK"))).clicked();
                cancel = ui.button(crate::tl!("Cancel")).clicked();
                if st.linked {
                    remove = ui.button(crate::tl!("Remove Link")).clicked();
                }
            });
        });
    let handles: Vec<String> = st.handles.iter().map(|h| h.hex()).collect();
    let r = if ok {
        Some(app.run("hyperlink", json!({ "handles": handles, "url": st.link.url, "target": st.link.target, "description": st.link.description })))
    } else if remove {
        Some(app.run("hyperlink", json!({ "handles": handles, "remove": true })))
    } else {
        None
    };
    if let Some(Err(e)) = r {
        app.session.echo(e);
    }
    if ok || remove || cancel || !*open {
        ctx.data_mut(|d| d.remove::<State>(id));
        *open = false;
    } else {
        ctx.data_mut(|d| d.insert_temp(id, st));
    }
}

/// The hyperlink of the object at `h`, if it has one.
fn link_at(app: &CadApp, h: Option<Handle>) -> Option<(Handle, Hyperlink)> {
    let h = h?;
    Some((h, hyperlink::read(&app.session.doc().ok()?.entity(h)?.common.xdata)?))
}

/// How following a link is asked for, as the platform says it.
fn follow_hint() -> &'static str {
    if cfg!(target_os = "macos") { "⌘-click to follow the link" } else { "Ctrl+click to follow the link" }
}

/// No command running: hovering a linked object shows its link; Ctrl+click (⌘-click) follows it.
/// `picked` is the object under the cursor; returns whether the click was taken.
pub fn hover_and_follow(app: &mut CadApp, ctx: &egui::Context, resp: &egui::Response, picked: Option<Handle>, clicked: bool, command: bool) -> bool {
    if app.session.running.is_some() || app.ui.dialog.is_some() {
        return false;
    }
    let Some((h, link)) = link_at(app, picked) else { return false };
    if !clicked {
        let text = format!("{}\n{}", link.label(), crate::i18n::t(follow_hint()));
        resp.clone().on_hover_text_at_pointer(text);
        if command {
            ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        return false;
    }
    if !command {
        return false;
    }
    follow(app, ctx, h);
    true
}

/// Follow object `h`'s link: a URL opens through the host, a named location of this drawing is
/// gone to by the engine.
pub fn follow(app: &mut CadApp, ctx: &egui::Context, h: Handle) {
    match app.run("hyperlink.open", json!({ "handle": h.hex() })) {
        Ok(r) => {
            if let Some(url) = r.get("url").and_then(serde_json::Value::as_str) {
                ctx.open_url(egui::OpenUrl::new_tab(url));
                app.session.echo(format!("Opening {url}"));
            }
        }
        Err(e) => app.session.echo(e),
    }
}
