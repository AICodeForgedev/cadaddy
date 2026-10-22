//! QVDRAWING and QVLAYOUT: a strip of thumbnails along the bottom of the drawing area, of the
//! open drawings or of the current drawing's model and layouts. Clicking one switches to it;
//! the ×, Esc or QVDRAWINGCLOSE / QVLAYOUTCLOSE close the strip. The thumbnails are drawn by the
//! engine (`cadaddy_engine::cmd::quickview`) and kept until their drawing changes.

use cadaddy_doc::Space;
use egui::{Color32, Rect, Sense, Stroke, pos2, vec2};
use serde_json::{Value, json};

use crate::CadApp;

/// Thumbnail size in pixels.
const W: u32 = 160;
const H: u32 = 112;
/// At most this many thumbnails in the strip.
const MAX_TILES: usize = 24;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Drawings,
    Layouts,
}

/// The strip while it is open, with the thumbnails drawn so far: (drawing uid, revision, space)
/// and its texture.
pub struct QuickView {
    pub kind: Kind,
    textures: Vec<((u64, u64, String), egui::TextureHandle)>,
}

impl QuickView {
    pub fn new(kind: Kind) -> Self {
        QuickView { kind, textures: Vec::new() }
    }
}

/// The quick view commands from anywhere: opening and closing the strip (the engine lists and
/// switches).
pub fn route(app: &mut CadApp, id: &str, params: &Value) -> Option<Result<Value, String>> {
    let kind = match id {
        "qvdrawing" => Kind::Drawings,
        "qvlayout" => Kind::Layouts,
        "qvdrawingclose" | "qvlayoutclose" => {
            app.quickview = None;
            return Some(app.session.execute(id, params).map_err(|e| e.user_message()));
        }
        _ => return None,
    };
    let r = app.session.execute(id, params).map_err(|e| e.user_message());
    // Switching from the strip keeps it open; the command alone opens it.
    if r.is_ok() && params.get("open").is_none_or(Value::is_null) && params.get("thumbnails").is_none() {
        app.quickview = Some(QuickView::new(kind));
    }
    Some(r)
}

fn space_name(sp: &Space) -> String {
    match sp {
        Space::Model => "Model".into(),
        Space::Paper(n) => n.clone(),
    }
}

/// The tiles: label, (drawing index, space), whether it is the current one.
fn tiles(app: &CadApp, kind: Kind) -> Vec<(String, usize, Space, bool)> {
    let s = &app.session;
    match kind {
        Kind::Drawings => s.docs.iter().enumerate().take(MAX_TILES).map(|(i, st)| (st.title.clone(), i, st.space.clone(), i == s.active)).collect(),
        Kind::Layouts => {
            let Ok(d) = s.doc() else { return Vec::new() };
            let mut l: Vec<_> = d.layouts.iter().collect();
            l.sort_by_key(|l| l.tab_order);
            let current = s.layout_space();
            std::iter::once(Space::Model)
                .chain(l.into_iter().map(|l| Space::Paper(l.name.clone())))
                .take(MAX_TILES)
                .map(|sp| (space_name(&sp), s.active, sp.clone(), sp == current))
                .collect()
        }
    }
}

fn texture(app: &mut CadApp, ctx: &egui::Context, doc: usize, sp: &Space) -> Option<egui::TextureHandle> {
    let st = app.session.docs.get(doc)?;
    let key = (st.uid, st.revision, space_name(sp));
    if let Some((_, t)) = app.quickview.as_ref()?.textures.iter().find(|(k, _)| *k == key) {
        return Some(t.clone());
    }
    let px = cadaddy_engine::cmd::quickview::thumbnail_rgba(st.drawing(), sp, W, H)?;
    let img = egui::ColorImage::from_rgba_premultiplied([W as usize, H as usize], &px);
    let t = ctx.load_texture(format!("qv-{}-{}-{}", key.0, key.1, key.2), img, egui::TextureOptions::LINEAR);
    let qv = app.quickview.as_mut()?;
    // A drawing's older thumbnails of the same space go.
    qv.textures.retain(|(k, _)| !(k.0 == key.0 && k.2 == key.2));
    qv.textures.push((key, t.clone()));
    Some(t)
}

/// The strip along the bottom of the drawing area `rect`.
pub fn show(app: &mut CadApp, ui: &mut egui::Ui, rect: Rect) {
    let Some(kind) = app.quickview.as_ref().map(|q| q.kind) else { return };
    let ctx = ui.ctx().clone();
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.quickview = None;
        return;
    }
    let list = tiles(app, kind);
    let (tw, th, gap) = (W as f32, H as f32 + 20.0, 10.0);
    let width = (list.len() as f32 * (tw + gap) + gap + 26.0).min(rect.width() - 20.0);
    let strip = Rect::from_center_size(pos2(rect.center().x, rect.bottom() - 60.0 - th / 2.0), vec2(width, th + 2.0 * gap));
    let bg = ui.interact(strip, ui.id().with("quickview"), Sense::click());
    crate::icons::describe_control(ui, &bg, if kind == Kind::Drawings { "Quick View Drawings" } else { "Quick View Layouts" }, None, false);
    let p = ui.painter_at(rect);
    p.rect_filled(strip, 6.0, Color32::from_rgba_unmultiplied(0x22, 0x27, 0x2e, 235));
    let text = Color32::from_rgb(0xe0, 0xe4, 0xea);
    let mut pick: Option<(usize, Space)> = None;
    for (i, (label, doc, sp, current)) in list.iter().enumerate() {
        let r = Rect::from_min_size(pos2(strip.left() + gap + i as f32 * (tw + gap), strip.top() + gap), vec2(tw, th));
        if r.right() > strip.right() - 26.0 {
            break;
        }
        let img = Rect::from_min_size(r.min, vec2(tw, H as f32));
        let resp = ui.interact(r, ui.id().with(("quickview-tile", i)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
        crate::icons::describe_control(ui, &resp, label, None, *current);
        match texture(app, &ctx, *doc, sp) {
            Some(t) => {
                p.image(t.id(), img, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            }
            None => {
                p.rect_filled(img, 0.0, Color32::from_rgb(33, 40, 48));
            }
        }
        let edge = if *current {
            Stroke::new(2.0, Color32::from_rgb(0x4d, 0x9c, 0xf5))
        } else if resp.hovered() {
            Stroke::new(1.5, Color32::from_rgb(0xb0, 0xb8, 0xc4))
        } else {
            Stroke::new(1.0, Color32::from_rgb(0x55, 0x5d, 0x68))
        };
        p.rect_stroke(img, 0.0, edge, egui::StrokeKind::Outside);
        let shown: String = label.chars().take(24).collect();
        p.text(pos2(r.center().x, r.bottom() - 9.0), egui::Align2::CENTER_CENTER, shown, crate::theme::small(), text);
        if resp.clicked() {
            pick = Some((*doc, sp.clone()));
        }
    }
    // The ×.
    let x = Rect::from_center_size(pos2(strip.right() - 14.0, strip.top() + 14.0), vec2(16.0, 16.0));
    let close = ui.interact(x, ui.id().with("quickview-close"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
    crate::icons::describe_control(ui, &close, "Close", None, false);
    let s = Stroke::new(1.4, if close.hovered() { Color32::WHITE } else { text });
    p.line_segment([x.center() + vec2(-4.0, -4.0), x.center() + vec2(4.0, 4.0)], s);
    p.line_segment([x.center() + vec2(-4.0, 4.0), x.center() + vec2(4.0, -4.0)], s);
    if close.clicked() {
        app.quickview = None;
        return;
    }
    if let Some((doc, sp)) = pick {
        let r = match kind {
            Kind::Drawings => app.run("qvdrawing", json!({ "open": doc })),
            Kind::Layouts => app.run("qvlayout", json!({ "open": space_name(&sp) })),
        };
        if let Err(e) = r {
            app.session.echo(e);
        }
    }
}
