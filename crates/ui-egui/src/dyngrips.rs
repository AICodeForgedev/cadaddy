//! Dynamic block grips on the canvas, as AutoCAD shows them on a selected dynamic block
//! reference: an arrow at a distance's end (drag it to change the distance), a square at a
//! position (drag it to move it) and a down-pointing triangle for a list (click it for the
//! visibility states or lookup values). Dragging runs the engine's dynamic grip edit
//! ([`cadaddy_engine::Session::start_dyn_grip`]); a list choice runs `blocks.reference.set`.
//!
//! Selected associative arrays show their grips here too: a square base (and polar centre) and
//! arrows for counts and spacings, named under the cursor with their option. A click starts the
//! engine's array grip drag ([`cadaddy_engine::Session::start_array_grip`]).

use std::sync::Arc;

use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, vec2};
use serde_json::json;

use crate::CadApp;
use crate::canvas::Xf;
use crate::theme::Tokens;
use cadaddy_engine::cmd::array::grips::{ArrayGrip, GripId, GripLook};
use cadaddy_engine::doc::Handle;
use cadaddy_engine::doc::dynblock::{DynGrip, GripShape};

/// Most selected references whose grips are shown.
const MAX_REFERENCES: usize = 200;

/// The dynamic grips of the selection, worked out once per drawing revision and selection.
#[derive(Clone, Default)]
pub struct Cache {
    key: (u64, u64, Vec<Handle>),
    grips: Arc<Vec<(Handle, DynGrip)>>,
    /// The selected associative arrays' grips (the ones a plan view can drag).
    arrays: Arc<Vec<(Handle, ArrayGrip)>>,
    /// The array grip being dragged.
    array_hot: Option<(Handle, GripId)>,
}

/// A special grip under the cursor: a dynamic block grip or an array grip.
#[derive(Clone)]
pub(crate) enum Hit {
    Dyn(Handle, DynGrip),
    Array(Handle, ArrayGrip),
}

/// The selected references' dynamic grips (cached).
pub(crate) fn of_selection(app: &mut CadApp) -> Arc<Vec<(Handle, DynGrip)>> {
    let sel = app.session.selection();
    let Ok(st) = app.session.state() else { return Arc::default() };
    let key = (st.uid, st.revision, sel.clone());
    if app.canvas.dyn_grips.key != key {
        let grips: Vec<(Handle, DynGrip)> = sel
            .iter()
            .filter(|h| matches!(st.doc.entity(**h).map(|e| &e.kind), Some(cadaddy_engine::doc::EntityKind::Insert(_))))
            .filter(|h| st.doc.entity(**h).is_some_and(|e| !st.doc.layer(&e.common.layer).is_some_and(|l| l.locked)))
            .take(MAX_REFERENCES)
            .flat_map(|h| cadaddy_engine::doc::dynblock::grips(&st.doc, *h).into_iter().map(move |g| (*h, g)))
            .collect();
        let arrays: Vec<(Handle, ArrayGrip)> = sel
            .iter()
            .filter(|h| st.doc.entity(**h).is_some_and(|e| !st.doc.layer(&e.common.layer).is_some_and(|l| l.locked)))
            .take(MAX_REFERENCES)
            .flat_map(|h| cadaddy_engine::cmd::array::grips::array_grips(&st.doc, *h).into_iter().filter(|g| g.plan).map(move |g| (*h, g)))
            .collect();
        let array_hot = app.canvas.dyn_grips.array_hot;
        app.canvas.dyn_grips = Cache { key, grips: Arc::new(grips), arrays: Arc::new(arrays), array_hot };
    }
    app.canvas.dyn_grips.grips.clone()
}

/// The selected arrays' grips (cached with [`of_selection`]).
fn arrays_of_selection(app: &mut CadApp) -> Arc<Vec<(Handle, ArrayGrip)>> {
    let _ = of_selection(app);
    app.canvas.dyn_grips.arrays.clone()
}

/// The screen outline of array grip `g` (size `gs`): a square, or a triangle pointing along it.
fn array_outline(xf: &Xf, g: &ArrayGrip, gs: f32) -> Vec<Pos2> {
    let c = xf.w2s(g.at);
    match g.look {
        GripLook::Square => vec![c + vec2(-gs, -gs), c + vec2(gs, -gs), c + vec2(gs, gs), c + vec2(-gs, gs)],
        GripLook::Arrow => {
            let ahead = xf.w2s(g.at + g.dir) - c;
            let u = if ahead.length() > 1e-6 { ahead / ahead.length() } else { vec2(1.0, 0.0) };
            let n = vec2(-u.y, u.x);
            // Longer than wide, so a diagonal arrow (the far-corner count grip) still reads as one.
            let (back, tip, half) = (gs * 0.8, gs * 1.8, gs * 0.75);
            vec![c - u * back + n * half, c + u * tip, c - u * back - n * half]
        }
    }
}

fn array_hit(xf: &Xf, g: &ArrayGrip, gs: f32, p: Pos2) -> bool {
    Rect::from_points(&array_outline(xf, g, gs)).expand(3.0).contains(p)
}

/// The name of array grip `g` and its option, as the cursor shows them.
fn array_label(g: &ArrayGrip) -> String {
    let name = crate::i18n::t(g.grip.label());
    match g.option {
        Some((k, _)) => format!("{name}  ·  {}", crate::i18n::t(k)),
        None => name.to_string(),
    }
}

/// The screen outline of grip `g` (size `gs`, half the side of a square grip).
fn outline(xf: &Xf, g: &DynGrip, gs: f32) -> Vec<Pos2> {
    let c = xf.w2s(g.at);
    match g.shape {
        GripShape::Linear => {
            // The arrow points the way the distance grows, on screen.
            let ahead = xf.w2s(g.at + g.dir) - c;
            let u = if ahead.length() > 1e-6 { ahead / ahead.length() } else { vec2(1.0, 0.0) };
            let n = vec2(-u.y, u.x);
            let (len, half) = (gs * 1.6, gs * 1.1);
            vec![c - u * (len * 0.5) + n * half, c + u * (len * 0.8), c - u * (len * 0.5) - n * half]
        }
        GripShape::Point => {
            vec![c + vec2(-gs, -gs), c + vec2(gs, -gs), c + vec2(gs, gs), c + vec2(-gs, gs)]
        }
        GripShape::List => {
            let w = gs * 1.3;
            vec![c + vec2(-w, -w * 0.6), c + vec2(w, -w * 0.6), c + vec2(0.0, w * 0.9)]
        }
    }
}

/// Whether `p` is on grip `g` (its box, a little larger, as for other grips).
fn hit(xf: &Xf, g: &DynGrip, gs: f32, p: Pos2) -> bool {
    let r = Rect::from_points(&outline(xf, g, gs)).expand(3.0);
    r.contains(p)
}

/// Draw the grips of the selected dynamic references; the one under the cursor warm, the one
/// being dragged hot.
pub(crate) fn draw(app: &mut CadApp, painter: &Painter, xf: &Xf, rect: Rect, hover: Option<Pos2>) {
    let t = Tokens::get();
    let gs = app.session.settings.gripsize as f32;
    let hot = app.canvas.dyn_hot;
    for (h, g) in of_selection(app).iter() {
        if !rect.contains(xf.w2s(g.at)) {
            continue;
        }
        let pts = outline(xf, g, gs);
        let is_hot = hot.as_ref().is_some_and(|(hh, n, p)| hh == h && *n == g.node && *p == g.part);
        let warm = !is_hot && hover.is_some_and(|p| hit(xf, g, gs, p));
        let fill = if is_hot {
            t.grip_hot
        } else if warm {
            Color32::from_rgb(0xff, 0x7f, 0x9f)
        } else {
            t.grip_dyn
        };
        painter.add(Shape::convex_polygon(pts, fill, Stroke::new(1.0, Color32::from_rgb(0x10, 0x30, 0x80))));
    }
    // Array grips; the hot one only while its drag runs.
    if app.session.running.as_ref().is_none_or(|r| r.id != "grip") {
        app.canvas.dyn_grips.array_hot = None;
    }
    let hot = app.canvas.dyn_grips.array_hot;
    let mut label = None;
    for (h, g) in arrays_of_selection(app).iter() {
        if !rect.contains(xf.w2s(g.at)) {
            continue;
        }
        let pts = array_outline(xf, g, gs);
        let is_hot = hot.is_some_and(|(hh, id)| hh == *h && id == g.grip);
        let warm = !is_hot && hover.is_some_and(|p| array_hit(xf, g, gs, p));
        let fill = if is_hot {
            t.grip_hot
        } else if warm {
            Color32::from_rgb(0xff, 0x7f, 0x9f)
        } else {
            t.grip
        };
        painter.add(Shape::convex_polygon(pts, fill, Stroke::new(1.0, Color32::from_rgb(0x10, 0x30, 0x80))));
        if warm && label.is_none() {
            label = Some((xf.w2s(g.at), array_label(g)));
        }
    }
    if let Some((at, text)) = label {
        let galley = painter.layout_no_wrap(text, crate::theme::small(), Color32::from_rgb(0x20, 0x20, 0x20));
        let r = Rect::from_min_size(at + vec2(gs + 8.0, gs + 8.0), galley.size() + vec2(8.0, 4.0));
        painter.rect_filled(r, 2.0, Color32::from_rgb(0xf4, 0xf4, 0xf4));
        painter.galley(r.min + vec2(4.0, 2.0), galley, Color32::BLACK);
    }
}

/// The dynamic block grip or array grip under `hover`, if any.
pub(crate) fn at(app: &mut CadApp, xf: &Xf, hover: Option<Pos2>) -> Option<Hit> {
    let p = hover?;
    let gs = app.session.settings.gripsize as f32;
    if let Some((h, g)) = of_selection(app).iter().find(|(_, g)| hit(xf, g, gs, p)).cloned() {
        return Some(Hit::Dyn(h, g));
    }
    arrays_of_selection(app).iter().find(|(_, g)| array_hit(xf, g, gs, p)).cloned().map(|(h, g)| Hit::Array(h, g))
}

/// A click on a dynamic grip (drag it, or open its list) or an array grip (drag it).
pub(crate) fn click(app: &mut CadApp, hit: Hit) {
    let (h, g) = match hit {
        Hit::Dyn(h, g) => (h, g),
        Hit::Array(h, g) => {
            app.canvas.hot_grips.clear();
            match app.session.start_array_grip(h, g.grip) {
                Ok(()) => app.canvas.dyn_grips.array_hot = Some((h, g.grip)),
                Err(e) => app.session.echo(e.to_string()),
            }
            return;
        }
    };
    if g.shape == GripShape::List {
        app.canvas.dyn_menu = Some((h, g));
        app.canvas.dyn_menu_fresh = true;
        return;
    }
    app.canvas.hot_grips.clear();
    match app.session.start_dyn_grip(h, g.node, g.part) {
        Ok(()) => app.canvas.dyn_hot = Some((h, g.node, g.part)),
        Err(e) => app.session.echo(e.to_string()),
    }
}

/// The open list of a list grip: its choices under the grip, the current one checked. A choice
/// sets the property (one undo step); a click elsewhere closes it.
pub(crate) fn menu(app: &mut CadApp, ctx: &egui::Context, xf: &Xf) {
    let Some((h, g)) = app.canvas.dyn_menu.clone() else { return };
    let current = app
        .session
        .state()
        .ok()
        .and_then(|st| cadaddy_engine::doc::dynblock::custom_properties(&st.doc, h))
        .and_then(|(_, ps)| ps.into_iter().find(|p| p.name.eq_ignore_ascii_case(&g.property)))
        .map(|p| p.value)
        .unwrap_or_default();
    let gs = app.session.settings.gripsize as f32;
    let at = xf.w2s(g.at) + vec2(-gs, gs * 1.5);
    let mut chosen: Option<String> = None;
    let area = egui::Area::new(egui::Id::new(("dyn-grip-list", h, g.node))).order(egui::Order::Foreground).fixed_pos(at).show(ctx, |ui| {
        egui::Frame::popup(ui.style()).show(ui, |ui| {
            for c in &g.choices {
                if ui.selectable_label(c.eq_ignore_ascii_case(&current), c.as_str()).clicked() {
                    chosen = Some(c.clone());
                }
            }
        });
    });
    // The click that opened the list isn't a click elsewhere.
    let fresh = std::mem::take(&mut app.canvas.dyn_menu_fresh);
    let clicked_elsewhere = !fresh && ctx.input(|i| i.pointer.any_pressed()) && !area.response.contains_pointer();
    if let Some(value) = chosen {
        app.canvas.dyn_menu = None;
        if let Err(e) = app.run("blocks.reference.set", json!({ "handle": h.hex(), "name": g.property, "value": value })) {
            app.session.echo(e);
        }
    } else if clicked_elsewhere || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.canvas.dyn_menu = None;
    }
}
