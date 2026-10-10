//! Plot style resolution: a plot style table applied to the display list while it is built.
//!
//! Plots (PDF) and layouts shown with "Display plot styles" (screen, PNG and SVG) draw each
//! object with the plot style that applies to it. In a colour-dependent table that is the style
//! of the object's index colour; a true colour uses the style of its nearest index colour. The
//! style replaces or keeps the colour, turns it grey and screens it (toward the white paper),
//! and replaces the linetype and lineweight. Colour 7 prints black, as on paper.

use cadcraft_color::{Rgb, nearest_aci};
use cadcraft_doc::{Drawing, Linetype, PlotStyle, PlotStyleTable};

use crate::Ink;

/// A table ready for the builder: styles plus their resolved linetype overrides.
pub(crate) struct Styler {
    table: std::sync::Arc<PlotStyleTable>,
    /// Per style: `None` keeps the object's linetype; `Some(None)` draws continuous;
    /// `Some(Some(lt))` uses `lt`.
    linetypes: Vec<Option<Option<Linetype>>>,
}

impl Styler {
    pub(crate) fn new(d: &Drawing, table: std::sync::Arc<PlotStyleTable>) -> Styler {
        let mut library: Option<Vec<Linetype>> = None;
        let linetypes = table
            .styles
            .iter()
            .map(|s| {
                let name = s.linetype.as_deref()?;
                if name.eq_ignore_ascii_case("continuous") || name.eq_ignore_ascii_case("solid") {
                    return Some(None);
                }
                let found = d.linetype(name).cloned().or_else(|| {
                    let lib = library.get_or_insert_with(|| cadcraft_doc::library::standard_linetypes_for(d));
                    lib.iter().find(|l| l.name.eq_ignore_ascii_case(name)).cloned()
                })?;
                Some((!found.pattern.is_empty()).then_some(found))
            })
            .collect();
        Styler { table, linetypes }
    }

    /// Index of the style that applies to `ink`.
    fn index(&self, ink: Ink) -> Option<usize> {
        if !self.table.is_color_dependent() {
            return None;
        }
        let aci = if ink.aci == 0 { nearest_aci(ink.rgb) } else { ink.aci };
        let i = usize::from(aci).checked_sub(1)?;
        (i < self.table.styles.len()).then_some(i)
    }

    fn style(&self, ink: Ink) -> Option<&PlotStyle> {
        self.table.styles.get(self.index(ink)?)
    }

    /// `ink` and lineweight `lw` (mm) as the plot style prints them. `lineweights`: lineweights
    /// are drawn at all (a style's lineweight replaces the object's only then).
    pub(crate) fn apply(&self, ink: Ink, lw: f32, lineweights: bool) -> (Ink, f32) {
        let Some(s) = self.style(ink) else { return (ink, lw) };
        let lw = match s.lineweight {
            Some(w) if lineweights => f32::from(w.min(211)) / 100.0,
            _ => lw,
        };
        if s.keeps_color() {
            return (ink, lw);
        }
        // Colour 7 is black on the paper the style prints on.
        let base = if ink.aci7 { Rgb(0, 0, 0) } else { ink.rgb };
        (Ink { rgb: s.apply_color(base), aci7: false, ..ink }, lw)
    }

    /// The linetype override of the style that applies to `ink`: `None` keeps the object's,
    /// `Some(None)` draws continuous.
    pub(crate) fn linetype(&self, ink: Ink) -> Option<Option<Linetype>> {
        self.linetypes.get(self.index(ink)?)?.clone()
    }
}
