//! PDF Import and Convert PDF to DWG: pick a PDF, choose the page, scale and layers, convert.
//!
//! Convert PDF to DWG (application menu) opens the page as a new drawing — model space full size
//! at the scale, a layout showing it like the sheet — then asks where to save it as DWG, named
//! after the PDF. PDF Import (Insert tab, PDFIMPORT) brings the page into the current drawing.
//! Both run the PDFIMPORT command; its options are explained there.

use serde_json::{Value, json};

use crate::CadApp;

/// Scales offered, as AutoCAD lists them: (label, drawing inches per sheet inch; 0 = auto).
pub const SCALES: &[(&str, f64)] = &[
    ("Auto (the sheet's scale note)", 0.0),
    ("1:1 (the sheet in inches)", 1.0),
    ("1/16\" = 1'-0\"", 192.0),
    ("3/32\" = 1'-0\"", 128.0),
    ("1/8\" = 1'-0\"", 96.0),
    ("3/16\" = 1'-0\"", 64.0),
    ("1/4\" = 1'-0\"", 48.0),
    ("3/8\" = 1'-0\"", 32.0),
    ("1/2\" = 1'-0\"", 24.0),
    ("3/4\" = 1'-0\"", 16.0),
    ("1\" = 1'-0\"", 12.0),
    ("1 1/2\" = 1'-0\"", 8.0),
    ("3\" = 1'-0\"", 4.0),
    ("1\" = 10'", 120.0),
    ("1\" = 20'", 240.0),
    ("1\" = 30'", 360.0),
    ("1\" = 40'", 480.0),
    ("1\" = 50'", 600.0),
    ("1\" = 100'", 1200.0),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layers {
    /// The PDF's layers, else one per colour and line weight.
    Pdf,
    /// One per colour and line weight.
    Pen,
    /// PDF_Geometry, PDF_Solid Fills, PDF_Text.
    Object,
}

#[derive(Clone, Debug)]
pub struct PdfDialog {
    pub path: String,
    /// Convert PDF to DWG (a new drawing, then Save As DWG); otherwise PDF Import here.
    pub convert: bool,
    pub page: u32,
    /// Index into [`SCALES`].
    pub scale: usize,
    pub layers: Layers,
    pub white_fills: bool,
    pub text: bool,
    pub join: bool,
    pub layout: bool,
    /// Raster images (logos, photos, scans) as IMAGE objects with PNG files.
    pub images: bool,
}

impl PdfDialog {
    pub fn new(path: String, convert: bool) -> Self {
        PdfDialog { path, convert, page: 1, scale: 0, layers: Layers::Pdf, white_fills: false, text: true, join: true, layout: true, images: true }
    }

    /// The PDFIMPORT parameters.
    pub fn params(&self) -> Value {
        let scale = SCALES.get(self.scale).map_or(0.0, |s| s.1);
        json!({
            "path": self.path,
            "page": self.page.max(1),
            "scale": if scale > 0.0 { json!(scale) } else { json!("auto") },
            "layers": match self.layers { Layers::Pdf => "pdf", Layers::Pen => "pen", Layers::Object => "object" },
            "whiteFills": self.white_fills,
            "text": self.text,
            "join": self.join,
            "layout": self.layout,
            "images": self.images,
            "newDrawing": self.convert,
        })
    }
}

/// Start Convert PDF to DWG (`convert`) or PDF Import: pick the PDF, then show the options.
pub fn begin(app: &mut CadApp, convert: bool) -> Result<Value, String> {
    let Some(pick) = app.services.pick_pdf.as_ref() else {
        return Err("PDF Import needs a file picker; type PDFIMPORT with a path instead".into());
    };
    let Some(path) = pick() else { return Ok(Value::Null) };
    app.pdf = Some(PdfDialog::new(path, convert));
    app.ui.dialog = Some("pdfimport".into());
    Ok(Value::Null)
}

/// Run the conversion with the dialog's choices; for Convert PDF to DWG, ask where to save first
/// (the image files go in a folder beside the drawing), then save as DWG there.
pub fn run(app: &mut CadApp, d: &PdfDialog) -> Result<Value, String> {
    let stem = std::path::Path::new(&d.path).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Drawing".into());
    let target = if d.convert {
        app.services
            .pick_save
            .as_ref()
            .and_then(|f| f(&format!("{stem}.dwg")))
            .map(|p| if std::path::Path::new(&p).extension().is_none() { format!("{p}.dwg") } else { p })
    } else {
        None
    };
    let mut params = d.params();
    if let Some(t) = &target {
        let t = std::path::Path::new(t);
        if let (Some(dir), Some(name)) = (t.parent(), t.file_stem()) {
            params["imageFolder"] = serde_json::json!(dir.join(format!("{} images", name.to_string_lossy())).to_string_lossy());
        }
    }
    let report = app.session.execute("pdfimport", &params).map_err(|e| e.to_string())?;
    let objects = report["entities"].as_u64().unwrap_or(0);
    let scale = report["scale"].as_f64().unwrap_or(1.0);
    let joined = report["stats"]["pieces joined"].as_u64().unwrap_or(0);
    let mut summary = format!("{objects} objects at 1:{scale} ({})", report["scaleSource"].as_str().unwrap_or(""));
    if joined > 0 {
        summary.push_str(&format!("; {joined} pieces joined"));
    }
    let images = report["imageFiles"].as_u64().unwrap_or(0);
    if images > 0 {
        summary.push_str(&format!("; {images} image file(s) in {}", report["imageFolder"].as_str().unwrap_or("")));
    }
    if d.convert {
        let saved = match target {
            Some(p) => {
                app.session.execute("saveas", &crate::menus::saveas_params(&app.ui, &app.session, &p)).map_err(|e| e.to_string())?;
                Some(p)
            }
            None => None,
        };
        app.set_status(match &saved {
            Some(p) => format!("Converted {stem}.pdf: {summary}. Saved {p}"),
            None => format!("Converted {stem}.pdf: {summary}. Not saved yet: Save As to keep it"),
        });
    } else {
        app.set_status(format!("PDF Import: {summary}"));
    }
    app.canvas.zoom_pending = true;
    Ok(report)
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let Some(mut d) = app.pdf.clone() else {
        *open = false;
        return;
    };
    let title = if d.convert { crate::tl!("Convert PDF to DWG") } else { crate::tl!("PDF Import") };
    let mut go = false;
    let mut cancel = false;
    egui::Window::new(title)
        .id(egui::Id::new("pdf_import_dialog"))
        .collapsible(false)
        .resizable(false)
        .open(open)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .show(ctx, |ui| {
            let file = std::path::Path::new(&d.path).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
            ui.label(egui::RichText::new(file).strong());
            ui.add_space(6.0);
            egui::Grid::new("pdf_import_grid").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                ui.label(crate::tl!("Page"));
                ui.add(egui::DragValue::new(&mut d.page).range(1..=9999));
                ui.end_row();
                ui.label(crate::tl!("Scale"));
                let shown = SCALES.get(d.scale).map_or("", |s| s.0);
                egui::ComboBox::from_id_salt("pdf_scale").selected_text(crate::i18n::t(shown)).width(230.0).show_ui(ui, |ui| {
                    for (i, (label, _)) in SCALES.iter().enumerate() {
                        ui.selectable_value(&mut d.scale, i, crate::i18n::t(label));
                    }
                });
                ui.end_row();
                ui.label(crate::tl!("Layers"));
                ui.vertical(|ui| {
                    ui.radio_value(&mut d.layers, Layers::Pdf, crate::tl!("PDF layers (else by colour and line weight)"));
                    ui.radio_value(&mut d.layers, Layers::Pen, crate::tl!("By colour and line weight"));
                    ui.radio_value(&mut d.layers, Layers::Object, crate::tl!("Object layers (PDF_Geometry, PDF_Text, …)"));
                });
                ui.end_row();
            });
            ui.add_space(4.0);
            ui.checkbox(&mut d.text, crate::tl!("Import text"));
            ui.checkbox(&mut d.join, crate::tl!("Join line pieces and make circles"));
            ui.checkbox(&mut d.white_fills, crate::tl!("Keep white fills (masks)"));
            ui.checkbox(&mut d.images, crate::tl!("Import raster images (saved as PNG files)"));
            if d.convert {
                ui.checkbox(&mut d.layout, crate::tl!("Add a layout that plots like the sheet"));
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let label = if d.convert { crate::tl!("Convert and Save as DWG…") } else { crate::tl!("Import") };
                if ui.button(label).clicked() {
                    go = true;
                }
                if ui.button(crate::tl!("Cancel")).clicked() {
                    cancel = true;
                }
            });
        });
    if go {
        *open = false;
        app.pdf = None;
        if let Err(e) = run(app, &d) {
            app.session.echo(format!("PDF Import: {e}"));
            app.set_status(format!("PDF Import failed: {e}"));
        }
        return;
    }
    if cancel || !*open {
        *open = false;
        app.pdf = None;
        return;
    }
    app.pdf = Some(d);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params_carry_the_choices() {
        let mut d = PdfDialog::new("C:/x/E-4.pdf".into(), true);
        assert_eq!(d.params()["scale"], "auto");
        d.scale = SCALES.iter().position(|s| s.1 == 96.0).unwrap_or(0);
        d.layers = Layers::Pen;
        let p = d.params();
        assert_eq!(p["scale"], 96.0);
        assert_eq!(p["layers"], "pen");
        assert_eq!(p["newDrawing"], true);
        assert_eq!(p["whiteFills"], false);
    }
}
