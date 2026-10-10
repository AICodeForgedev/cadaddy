//! Plot style tables: how objects print.
//!
//! A colour-dependent table (`.ctb`) has one plot style per index colour 1..=255: an object
//! prints with the style of its colour. A named table (`.stb`) holds styles that layers and
//! objects name directly. Each style can replace the object's colour, fade it (screening), turn
//! it grey, and replace its linetype and lineweight.
//!
//! CADCraft's built-in tables are our own definitions, not copies of any vendor's files:
//! [`DEFAULT_CTB`] keeps the object's properties, [`MONOCHROME_CTB`] prints everything black
//! and [`GRAYSCALE_CTB`] prints every colour as the grey of its luminance.

use cadcraft_color::Rgb;
use serde::{Deserialize, Serialize};

/// Built-in colour-dependent table that prints objects with their own properties.
pub const DEFAULT_CTB: &str = "default.ctb";
/// Built-in colour-dependent table that prints everything black.
pub const MONOCHROME_CTB: &str = "monochrome.ctb";
/// Built-in colour-dependent table that prints every colour as a grey of its luminance.
pub const GRAYSCALE_CTB: &str = "grayscale.ctb";

/// The names of the built-in tables.
pub const BUILTIN_PLOT_STYLE_TABLES: &[&str] = &[DEFAULT_CTB, MONOCHROME_CTB, GRAYSCALE_CTB];

/// Number of styles in a colour-dependent table (one per index colour 1..=255).
pub const CTB_STYLES: usize = 255;

/// Line end style of a plot style.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EndStyle {
    #[default]
    Object,
    Butt,
    Square,
    Round,
    Diamond,
}

/// Line join style of a plot style.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum JoinStyle {
    #[default]
    Object,
    Miter,
    Bevel,
    Round,
    Diamond,
}

/// Fill style of a plot style (for filled areas).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FillStyle {
    #[default]
    Object,
    Solid,
    Checkerboard,
    Crosshatch,
    Diamonds,
    HorizontalBars,
    SlantLeft,
    SlantRight,
    SquareDots,
    VerticalBars,
}

/// One plot style.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PlotStyle {
    pub name: String,
    pub description: String,
    /// Output colour; `None` uses the object's colour.
    pub color: Option<Rgb>,
    /// Print the colour as the grey of its luminance.
    pub grayscale: bool,
    /// Ink intensity in percent, 0 (white) to 100 (full colour).
    pub screening: u8,
    /// Linetype name; `None` uses the object's linetype.
    pub linetype: Option<String>,
    /// Lineweight in hundredths of a millimetre (0..=211); `None` uses the object's lineweight.
    pub lineweight: Option<u16>,
    pub end: EndStyle,
    pub join: JoinStyle,
    pub fill: FillStyle,
}

impl Default for PlotStyle {
    fn default() -> Self {
        PlotStyle::object("")
    }
}

impl PlotStyle {
    /// A style that keeps every object property.
    pub fn object(name: &str) -> PlotStyle {
        PlotStyle {
            name: name.into(),
            description: String::new(),
            color: None,
            grayscale: false,
            screening: 100,
            linetype: None,
            lineweight: None,
            end: EndStyle::Object,
            join: JoinStyle::Object,
            fill: FillStyle::Object,
        }
    }
    /// Whether the style leaves the colour alone.
    pub fn keeps_color(&self) -> bool {
        self.color.is_none() && !self.grayscale && self.screening >= 100
    }
    /// The printed colour of an object drawn in `rgb`: the style's colour (else `rgb`), turned
    /// grey when asked, then screened toward white.
    pub fn apply_color(&self, rgb: Rgb) -> Rgb {
        let c = self.color.unwrap_or(rgb);
        let c = if self.grayscale {
            let g = (c.luma().clamp(0.0, 1.0) * 255.0).round() as u8;
            Rgb(g, g, g)
        } else {
            c
        };
        let s = u16::from(self.screening.min(100));
        // Screening mixes the ink with the white paper: 100 % = full ink, 0 % = white.
        let screen = |v: u8| (255 - (255 - u16::from(v)) * s / 100) as u8;
        Rgb(screen(c.0), screen(c.1), screen(c.2))
    }
}

/// Whether a table maps colours or names to styles.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PlotStyleKind {
    /// Colour-dependent (`.ctb`): style `i - 1` applies to index colour `i`.
    #[default]
    ColorDependent,
    /// Named (`.stb`): styles are picked by name.
    Named,
}

/// A plot style table.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PlotStyleTable {
    /// File name, such as `monochrome.ctb`.
    pub name: String,
    pub description: String,
    pub kind: PlotStyleKind,
    pub styles: Vec<PlotStyle>,
}

impl PlotStyleTable {
    /// A colour-dependent table whose style for index colour `i` is `f(i)` (else object
    /// properties), named `Color_i`.
    pub fn color_dependent(name: &str, description: &str, f: impl Fn(u8) -> Option<PlotStyle>) -> PlotStyleTable {
        let styles = (1..=255u8)
            .map(|i| {
                let label = format!("Color_{i}");
                let mut s = f(i).unwrap_or_else(|| PlotStyle::object(&label));
                s.name = label;
                s
            })
            .collect();
        PlotStyleTable { name: name.into(), description: description.into(), kind: PlotStyleKind::ColorDependent, styles }
    }

    /// A built-in table by name (case-insensitive; the extension may be left out).
    pub fn builtin(name: &str) -> Option<PlotStyleTable> {
        let n = name.trim().to_ascii_lowercase();
        let n = if n.ends_with(".ctb") { n } else { format!("{n}.ctb") };
        match n.as_str() {
            DEFAULT_CTB => Some(PlotStyleTable::color_dependent(DEFAULT_CTB, "Object colours, linetypes and lineweights", |_| None)),
            MONOCHROME_CTB => Some(PlotStyleTable::color_dependent(MONOCHROME_CTB, "Every colour prints black", |_| {
                Some(PlotStyle { color: Some(Rgb(0, 0, 0)), ..PlotStyle::default() })
            })),
            GRAYSCALE_CTB => Some(PlotStyleTable::color_dependent(GRAYSCALE_CTB, "Every colour prints as the grey of its luminance", |_| {
                Some(PlotStyle { grayscale: true, ..PlotStyle::default() })
            })),
            _ => None,
        }
    }

    /// Whether this table is colour-dependent.
    pub fn is_color_dependent(&self) -> bool {
        self.kind == PlotStyleKind::ColorDependent
    }

    /// The style for index colour `aci` (1..=255) of a colour-dependent table.
    pub fn by_color(&self, aci: u8) -> Option<&PlotStyle> {
        if !self.is_color_dependent() || aci == 0 {
            return None;
        }
        self.styles.get(usize::from(aci) - 1)
    }

    /// The style called `name` (case-insensitive).
    pub fn by_name(&self, name: &str) -> Option<&PlotStyle> {
        self.styles.iter().find(|s| s.name.eq_ignore_ascii_case(name))
    }
}

/// Whether a page setup's table name means "no plot style table".
pub fn no_plot_style_table(name: &str) -> bool {
    let n = name.trim();
    n.is_empty() || n.eq_ignore_ascii_case("none")
}

/// The plot style table a page setup names, if it is one we know; `None` for no table
/// (empty or "None") and for tables we don't have.
pub fn plot_style_table(_d: &crate::Drawing, name: &str) -> Option<PlotStyleTable> {
    if no_plot_style_table(name) {
        return None;
    }
    PlotStyleTable::builtin(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_are_complete_color_dependent_tables() {
        for n in BUILTIN_PLOT_STYLE_TABLES {
            let t = PlotStyleTable::builtin(n).unwrap();
            assert_eq!(&t.name, n);
            assert!(t.is_color_dependent());
            assert_eq!(t.styles.len(), CTB_STYLES);
            assert_eq!(t.by_color(1).unwrap().name, "Color_1");
            assert_eq!(t.by_color(255).unwrap().name, "Color_255");
            assert!(t.by_color(0).is_none());
        }
        assert!(PlotStyleTable::builtin("Monochrome").is_some());
        assert!(PlotStyleTable::builtin("nope.ctb").is_none());
        assert!(no_plot_style_table("") && no_plot_style_table(" None ") && !no_plot_style_table("monochrome.ctb"));
    }

    #[test]
    fn colour_transforms() {
        let mono = PlotStyleTable::builtin(MONOCHROME_CTB).unwrap();
        assert_eq!(mono.by_color(1).unwrap().apply_color(Rgb(255, 0, 0)), Rgb(0, 0, 0));
        let gray = PlotStyleTable::builtin(GRAYSCALE_CTB).unwrap();
        let g = gray.by_color(3).unwrap().apply_color(Rgb(0, 255, 0));
        assert!(g.0 == g.1 && g.1 == g.2 && g.0 > 128, "{g:?}");
        let half = PlotStyle { color: Some(Rgb(0, 0, 0)), screening: 50, ..PlotStyle::default() };
        assert_eq!(half.apply_color(Rgb(9, 9, 9)), Rgb(128, 128, 128));
        let none = PlotStyle { screening: 0, ..PlotStyle::default() };
        assert_eq!(none.apply_color(Rgb(10, 20, 30)), Rgb(255, 255, 255));
        assert!(PlotStyle::default().keeps_color() && !half.keeps_color());
    }

    #[test]
    fn json_round_trip_with_defaults() {
        let t = PlotStyleTable::builtin(GRAYSCALE_CTB).unwrap();
        let back: PlotStyleTable = serde_json::from_value(serde_json::to_value(&t).unwrap()).unwrap();
        assert_eq!(back, t);
        let s: PlotStyle = serde_json::from_value(serde_json::json!({"name": "Thick", "lineweight": 50})).unwrap();
        assert_eq!((s.screening, s.lineweight, s.color), (100, Some(50), None));
    }
}
