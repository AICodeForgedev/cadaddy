//! The Geometric Tolerance dialog (TOLERANCE, double-click on a frame): two lines of symbol,
//! tolerances and datums, the projected tolerance zone and the datum identifier. The symbols are
//! drawn from CADaddy's own geometry ([`cadaddy_engine::doc::symbol_prims`]). OK places a new frame
//! (TOLERANCE asks for the location) or changes the frame being edited (TOLERANCE.EDIT).

use cadaddy_engine::Input;
use cadaddy_engine::doc::{GdtDatum, GdtFrame, GdtLine, GdtPrim, GdtSymbol, GdtTol, Handle};
use egui::{Color32, Pos2, Rect, Sense, Stroke, vec2};
use serde_json::json;

use crate::CadApp;
use crate::theme::Tokens;

/// The characteristics offered in the symbol grid, as AutoCAD's dialog orders them.
const CHARACTERISTICS: [GdtSymbol; 14] = [
    GdtSymbol::Position,
    GdtSymbol::Concentricity,
    GdtSymbol::Symmetry,
    GdtSymbol::Parallelism,
    GdtSymbol::Perpendicularity,
    GdtSymbol::Angularity,
    GdtSymbol::Cylindricity,
    GdtSymbol::Flatness,
    GdtSymbol::Circularity,
    GdtSymbol::Straightness,
    GdtSymbol::SurfaceProfile,
    GdtSymbol::LineProfile,
    GdtSymbol::CircularRunout,
    GdtSymbol::TotalRunout,
];

/// Material conditions a click steps through: none, M, L, S.
const MATERIALS: [Option<GdtSymbol>; 4] = [None, Some(GdtSymbol::Mmc), Some(GdtSymbol::Lmc), Some(GdtSymbol::Rfs)];

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TolField {
    pub diameter: bool,
    pub value: String,
    pub material: Option<GdtSymbol>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DatumField {
    pub letter: String,
    pub material: Option<GdtSymbol>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LineFields {
    pub symbol: Option<GdtSymbol>,
    pub tol: [TolField; 2],
    pub datums: [DatumField; 3],
}

/// The dialog's fields while it is open.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToleranceDialog {
    pub lines: [LineFields; 2],
    pub height: String,
    pub projected: bool,
    pub datum_id: String,
    /// The frame being edited (double-click); `None` makes a new one.
    pub edit: Option<Handle>,
    /// The line whose symbol grid is open.
    picking: Option<usize>,
}

fn material_letter(m: Option<GdtSymbol>) -> Option<String> {
    m.map(|g| g.name().chars().next().unwrap_or('m').to_ascii_uppercase().to_string())
}

impl ToleranceDialog {
    /// The dialog filled from a frame.
    pub fn from_frame(f: &GdtFrame) -> Self {
        let mut d = ToleranceDialog::default();
        for (line, src) in d.lines.iter_mut().zip(&f.lines) {
            line.symbol = src.symbol;
            for (t, s) in line.tol.iter_mut().zip([&src.tol1, &src.tol2]) {
                if let Some(s) = s {
                    *t = TolField { diameter: s.diameter, value: s.value.clone(), material: s.material.as_deref().and_then(GdtSymbol::parse) };
                }
            }
            for (dt, s) in line.datums.iter_mut().zip(&src.datums) {
                *dt = DatumField { letter: s.letter.clone(), material: s.material.as_deref().and_then(GdtSymbol::parse) };
            }
        }
        d.height = f.height.clone();
        d.projected = f.projected;
        d.datum_id = f.datum_id.clone();
        d
    }

    /// The frame the fields describe.
    pub fn frame(&self) -> GdtFrame {
        let tol = |t: &TolField| {
            (t.diameter || !t.value.trim().is_empty() || t.material.is_some()).then(|| GdtTol {
                diameter: t.diameter,
                value: t.value.trim().to_string(),
                material: material_letter(t.material),
            })
        };
        let lines = self
            .lines
            .iter()
            .map(|l| GdtLine {
                symbol: l.symbol,
                tol1: tol(&l.tol[0]),
                tol2: tol(&l.tol[1]),
                datums: l
                    .datums
                    .iter()
                    .filter(|d| !d.letter.trim().is_empty() || d.material.is_some())
                    .map(|d| GdtDatum { letter: d.letter.trim().to_string(), material: material_letter(d.material) })
                    .collect(),
            })
            .filter(|l| l.symbol.is_some() || l.tol1.is_some() || l.tol2.is_some() || !l.datums.is_empty())
            .collect();
        GdtFrame { lines, height: self.height.trim().to_string(), projected: self.projected, datum_id: self.datum_id.trim().to_string() }
    }

    /// The frame string (DXF group 1).
    pub fn text(&self) -> String {
        cadaddy_engine::doc::compose_frame(&self.frame())
    }

    pub fn is_empty(&self) -> bool {
        let t = self.text();
        cadaddy_engine::doc::parse_frame(&t).iter().flatten().all(cadaddy_engine::doc::compartment_is_empty)
    }
}

/// TOLERANCE from the menu or typed: an empty dialog for a new frame.
pub fn open_new(app: &mut CadApp) {
    if app.session.state().is_err() {
        return;
    }
    let mut d = ToleranceDialog::default();
    d.lines[0].symbol = Some(GdtSymbol::Position);
    app.tol_dlg = Some(d);
    app.ui.dialog = Some("tolerance".into());
}

/// Double-click on a frame: the dialog with its contents.
pub fn open_edit(app: &mut CadApp, h: Handle) -> bool {
    let Some(text) = app.session.doc().ok().and_then(|d| d.entity(h)).and_then(|e| match &e.kind {
        cadaddy_engine::doc::EntityKind::Tolerance(t) => Some(t.text.clone()),
        _ => None,
    }) else {
        return false;
    };
    let mut d = ToleranceDialog::from_frame(&cadaddy_engine::cmd::tolerance::frame_from_text(&text));
    d.edit = Some(h);
    app.tol_dlg = Some(d);
    app.ui.dialog = Some("tolerance".into());
    true
}

/// OK: change the frame being edited, or start TOLERANCE with the frame so the location is asked.
pub fn apply(app: &mut CadApp, d: &ToleranceDialog) -> Result<(), String> {
    if d.is_empty() {
        return Err("The feature control frame is empty.".into());
    }
    let text = d.text();
    match d.edit {
        Some(h) => app.run("tolerance.edit", json!({ "handle": h.hex(), "text": text })).map(|_| ()),
        None => {
            app.session.start("tolerance").map_err(|e| e.to_string())?;
            app.session.input(Input::Text(text)).map_err(|e| e.to_string())
        }
    }
}

/// Draw `sym` (its unit-box geometry) inside `r`.
fn paint_symbol(p: &egui::Painter, r: Rect, sym: GdtSymbol, color: Color32) {
    let r = r.shrink(r.height() * 0.22);
    let side = r.width().min(r.height());
    let o = Pos2::new(r.center().x - side / 2.0, r.center().y + side / 2.0);
    let at = |v: cadaddy_engine::doc::geom::Vec2| Pos2::new(o.x + v.x as f32 * side, o.y - v.y as f32 * side);
    let stroke = Stroke::new(1.4, color);
    for prim in cadaddy_engine::doc::symbol_prims(sym) {
        match prim {
            GdtPrim::Line(pts) => {
                p.add(egui::Shape::line(pts.into_iter().map(at).collect(), stroke));
            }
            GdtPrim::Circle { center, radius } => {
                p.circle_stroke(at(center), radius as f32 * side, stroke);
            }
            GdtPrim::Arc { center, radius, start, end } => {
                let sweep = if end > start { end - start } else { end + std::f64::consts::TAU - start };
                let n = 16;
                let pts = (0..=n)
                    .map(|i| {
                        let a = start + sweep * f64::from(i) / f64::from(n);
                        at(center + cadaddy_engine::doc::geom::Vec2::from_angle(a) * radius)
                    })
                    .collect();
                p.add(egui::Shape::line(pts, stroke));
            }
            GdtPrim::Fill(tri) => {
                p.add(egui::Shape::convex_polygon(tri.iter().map(|v| at(*v)).collect(), color, Stroke::NONE));
            }
        }
    }
}

/// A square button showing `sym` (blank for `None`); returns whether it was clicked.
fn symbol_button(ui: &mut egui::Ui, sym: Option<GdtSymbol>, hint: &str) -> bool {
    let t = Tokens::get();
    let (r, resp) = ui.allocate_exact_size(vec2(26.0, 24.0), Sense::click());
    let p = ui.painter();
    let fill = if resp.hovered() { t.hover } else { t.field };
    p.rect(r, 3.0, fill, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
    if let Some(s) = sym {
        paint_symbol(p, r, s, t.text);
    }
    resp.on_hover_text(hint).clicked()
}

fn next_material(m: Option<GdtSymbol>) -> Option<GdtSymbol> {
    let i = MATERIALS.iter().position(|x| *x == m).unwrap_or(0);
    MATERIALS.get((i + 1) % MATERIALS.len()).copied().flatten()
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let Some(mut d) = app.tol_dlg.clone() else {
        *open = false;
        return;
    };
    let (mut ok, mut cancel) = (false, false);
    egui::Window::new(crate::tl!("Geometric Tolerance"))
        .id(egui::Id::new("tolerance_dialog"))
        .collapsible(false)
        .resizable(false)
        .open(open)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .show(ctx, |ui| {
            egui::Grid::new("tolerance_grid").num_columns(6).spacing([10.0, 6.0]).show(ui, |ui| {
                for h in ["Sym", "Tolerance 1", "Tolerance 2", "Datum 1", "Datum 2", "Datum 3"] {
                    ui.label(crate::i18n::t(h));
                }
                ui.end_row();
                for (li, line) in d.lines.iter_mut().enumerate() {
                    if symbol_button(ui, line.symbol, crate::tl!("Geometric characteristic")) {
                        d.picking = if d.picking == Some(li) { None } else { Some(li) };
                    }
                    for tol in line.tol.iter_mut() {
                        ui.horizontal(|ui| {
                            if symbol_button(ui, tol.diameter.then_some(GdtSymbol::Diameter), crate::tl!("Diameter")) {
                                tol.diameter = !tol.diameter;
                            }
                            ui.add(egui::TextEdit::singleline(&mut tol.value).desired_width(56.0));
                            if symbol_button(ui, tol.material, crate::tl!("Material condition")) {
                                tol.material = next_material(tol.material);
                            }
                        });
                    }
                    for datum in line.datums.iter_mut() {
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut datum.letter).desired_width(36.0));
                            if symbol_button(ui, datum.material, crate::tl!("Material condition")) {
                                datum.material = next_material(datum.material);
                            }
                        });
                    }
                    ui.end_row();
                }
            });
            // The symbol grid for the line being picked: the characteristics, then none.
            if let Some(li) = d.picking {
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    for s in CHARACTERISTICS {
                        if symbol_button(ui, Some(s), s.name())
                            && let Some(l) = d.lines.get_mut(li)
                        {
                            l.symbol = Some(s);
                            d.picking = None;
                        }
                    }
                    if symbol_button(ui, None, crate::tl!("No symbol"))
                        && let Some(l) = d.lines.get_mut(li)
                    {
                        l.symbol = None;
                        d.picking = None;
                    }
                });
            }
            ui.add_space(6.0);
            egui::Grid::new("tolerance_extra").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                ui.label(crate::tl!("Height"));
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut d.height).desired_width(70.0));
                    if symbol_button(ui, d.projected.then_some(GdtSymbol::Projected), crate::tl!("Projected Tolerance Zone")) {
                        d.projected = !d.projected;
                    }
                    ui.label(crate::tl!("Projected Tolerance Zone"));
                });
                ui.end_row();
                ui.label(crate::tl!("Datum Identifier"));
                ui.add(egui::TextEdit::singleline(&mut d.datum_id).desired_width(70.0));
                ui.end_row();
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ok = ui.button(crate::tl!("OK")).clicked();
                cancel = ui.button(crate::tl!("Cancel")).clicked();
            });
        });
    if ok {
        match apply(app, &d) {
            Ok(()) => {
                *open = false;
                app.tol_dlg = None;
            }
            Err(e) => {
                app.set_status(e);
                app.tol_dlg = Some(d);
            }
        }
        return;
    }
    if cancel || !*open {
        *open = false;
        app.tol_dlg = None;
        return;
    }
    app.tol_dlg = Some(d);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_and_frame_strings_go_both_ways() {
        let mut d = ToleranceDialog::default();
        d.lines[0].symbol = Some(GdtSymbol::Position);
        d.lines[0].tol[0] = TolField { diameter: true, value: "0.05".into(), material: Some(GdtSymbol::Mmc) };
        d.lines[0].datums[0] = DatumField { letter: "A".into(), material: None };
        d.lines[0].datums[1] = DatumField { letter: "B".into(), material: Some(GdtSymbol::Lmc) };
        d.lines[1].symbol = Some(GdtSymbol::Flatness);
        d.lines[1].tol[0].value = "0.01".into();
        d.height = "12".into();
        d.projected = true;
        d.datum_id = "C".into();
        let back = ToleranceDialog::from_frame(&cadaddy_engine::cmd::tolerance::frame_from_text(&d.text()));
        assert_eq!(back, ToleranceDialog { picking: None, ..d.clone() });
        assert!(!d.is_empty());
        assert!(ToleranceDialog::default().is_empty());
    }

    #[test]
    fn ok_places_a_new_frame_or_edits_the_one_double_clicked() {
        let mut app = CadApp::new(cadaddy_engine::Session::new(), crate::Services::default());
        open_new(&mut app);
        assert_eq!(app.ui.dialog.as_deref(), Some("tolerance"));
        let mut d = app.tol_dlg.clone().unwrap();
        d.lines[0].tol[0].value = "0.1".into();
        apply(&mut app, &d).unwrap();
        // TOLERANCE now asks only for the location.
        assert_eq!(app.session.current_prompt().map(|p| p.display()).as_deref(), Some("Enter tolerance location:"));
        app.session.input(Input::Point(cadaddy_engine::doc::geom::Vec2::new(2.0, 3.0))).unwrap();
        let h = app.session.doc().unwrap().model.iter().last().unwrap().handle;
        assert!(open_edit(&mut app, h));
        let mut e = app.tol_dlg.clone().unwrap();
        assert_eq!(e.lines[0].tol[0].value, "0.1");
        e.lines[0].tol[0].value = "0.2".into();
        apply(&mut app, &e).unwrap();
        let doc = app.session.doc().unwrap();
        let cadaddy_engine::doc::EntityKind::Tolerance(t) = &doc.entity(h).unwrap().kind else { panic!() };
        assert!(t.text.contains("0.2") && (t.insert.x, t.insert.y) == (2.0, 3.0), "{t:?}");
        // An empty frame is refused.
        assert!(apply(&mut app, &ToleranceDialog::default()).is_err());
    }
}
