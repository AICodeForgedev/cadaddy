//! PDF review markups (annotations another program added to a sheet) as MARKUPIMPORT reads
//! them: kind, where on the page (PDF points, origin bottom left), the comment, who and when.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MarkupKind {
    /// A cloud (cloudy polygon, rectangle or ellipse).
    Cloud,
    Rectangle,
    Ellipse,
    Polygon,
    Polyline,
    Line,
    /// A line or polyline ending in an arrowhead (at its first point).
    Arrow,
    /// Freehand pen strokes.
    Ink,
    /// A sticky note or a text box.
    Note,
    /// A text box with a leader to what it is about (`points`: the leader, arrow first).
    Callout,
    Stamp,
    /// Highlighted (or underlined, struck out) text: `strokes` hold each marked box.
    Highlight,
}

/// One markup.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Markup {
    pub kind: MarkupKind,
    /// Its outline or path (PDF points).
    pub points: Vec<[f64; 2]>,
    /// Ink strokes and highlighted boxes.
    #[serde(default)]
    pub strokes: Vec<Vec<[f64; 2]>>,
    /// The annotation's rectangle: x0, y0, x1, y1.
    pub rect: [f64; 4],
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub subject: String,
    #[serde(default)]
    pub color: Option<[u8; 3]>,
    #[serde(default)]
    pub date: String,
}

/// A page's markups and its size (points).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MarkupPage {
    pub width: f64,
    pub height: f64,
    pub markups: Vec<Markup>,
}

/// Reads page `page` (1-based) of a PDF's markups (installed by the host; PDFs are `io`'s).
pub type MarkupReader = fn(&[u8], u32) -> Result<MarkupPage, String>;
