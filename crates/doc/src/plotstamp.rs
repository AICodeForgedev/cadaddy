//! The plot stamp (PLOTSTAMP): a line of text plotted at a corner of each sheet saying what was
//! plotted, when and how: the drawing's name, the layout, the date and time, the device, the
//! paper size, the plot scale, and lines of the user's own. The settings are the drawing's (kept
//! with it, in .cdy and DXF); each layout's page setup says whether its plots carry the stamp
//! ([`crate::PageSetup::plot_stamp`]), and [`PlotStamp::model`] whether model space plots do.
//!
//! No login or computer name is ever stamped: a plot often leaves the office.

use serde::{Deserialize, Serialize};

/// Which corner of the sheet the stamp sits at.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StampCorner {
    #[default]
    BottomLeft,
    BottomRight,
    TopLeft,
    TopRight,
}

/// The plot stamp's settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PlotStamp {
    pub drawing_name: bool,
    pub layout_name: bool,
    pub date_time: bool,
    pub device_name: bool,
    pub paper_size: bool,
    pub plot_scale: bool,
    /// Text of the user's own, stamped after the fields.
    pub user_fields: Vec<String>,
    pub corner: StampCorner,
    /// From the corner, inward, millimetres (x, y).
    pub offset_mm: [f64; 2],
    /// The offset is from the paper's edge; otherwise from the printable area's.
    pub from_paper_edge: bool,
    /// One of the standard PDF fonts: Helvetica, Times or Courier.
    pub font: String,
    pub height_mm: f64,
    /// All on one line (fields separated by commas); otherwise two lines, the fields then the
    /// user's own.
    pub single_line: bool,
    /// Model space plots carry the stamp too.
    pub model: bool,
}

impl Default for PlotStamp {
    fn default() -> Self {
        PlotStamp {
            drawing_name: true,
            layout_name: true,
            date_time: true,
            device_name: true,
            paper_size: false,
            plot_scale: false,
            user_fields: Vec::new(),
            corner: StampCorner::BottomLeft,
            offset_mm: [2.0, 2.0],
            from_paper_edge: false,
            font: "Helvetica".into(),
            height_mm: 2.0,
            single_line: true,
            model: false,
        }
    }
}

/// Fonts a stamp may be written in.
pub const STAMP_FONTS: [&str; 3] = ["Helvetica", "Times", "Courier"];
/// Smallest and largest stamp text, millimetres.
pub const STAMP_HEIGHT_MM: (f64, f64) = (0.5, 20.0);
/// Most user fields, and characters in one.
pub const MAX_USER_FIELDS: usize = 8;
pub const MAX_FIELD_CHARS: usize = 200;
/// Largest offset from the corner, millimetres.
pub const MAX_STAMP_OFFSET_MM: f64 = 1000.0;

/// What a stamp says about one plotted sheet.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StampFacts {
    pub drawing: String,
    pub layout: String,
    pub date_time: String,
    pub device: String,
    pub paper: String,
    pub scale: String,
}

impl PlotStamp {
    /// The settings made safe to plot: a known font, a sane height and offset, short fields.
    pub fn sanitized(&self) -> PlotStamp {
        let mut s = self.clone();
        s.font = STAMP_FONTS.iter().find(|f| f.eq_ignore_ascii_case(self.font.trim())).unwrap_or(&STAMP_FONTS[0]).to_string();
        s.height_mm = if s.height_mm.is_finite() { s.height_mm.clamp(STAMP_HEIGHT_MM.0, STAMP_HEIGHT_MM.1) } else { 2.0 };
        for v in &mut s.offset_mm {
            *v = if v.is_finite() { v.clamp(-MAX_STAMP_OFFSET_MM, MAX_STAMP_OFFSET_MM) } else { 0.0 };
        }
        s.user_fields = s
            .user_fields
            .iter()
            .map(|f| f.chars().filter(|c| !c.is_control()).take(MAX_FIELD_CHARS).collect::<String>())
            .filter(|f| !f.trim().is_empty())
            .take(MAX_USER_FIELDS)
            .collect();
        s
    }

    /// The stamp's lines for one sheet (one line, or the fields then the user's own).
    pub fn lines(&self, f: &StampFacts) -> Vec<String> {
        let mut fields = Vec::new();
        for (on, v) in [
            (self.drawing_name, &f.drawing),
            (self.layout_name, &f.layout),
            (self.date_time, &f.date_time),
            (self.device_name, &f.device),
            (self.paper_size, &f.paper),
            (self.plot_scale, &f.scale),
        ] {
            if on && !v.trim().is_empty() {
                fields.push(v.trim().to_string());
            }
        }
        let user: Vec<String> = self.sanitized().user_fields;
        if self.single_line {
            let all: Vec<String> = fields.into_iter().chain(user).collect();
            if all.is_empty() { Vec::new() } else { vec![all.join(", ")] }
        } else {
            [fields.join(", "), user.join(", ")].into_iter().filter(|l| !l.is_empty()).collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> StampFacts {
        StampFacts {
            drawing: "Plan.dwg".into(),
            layout: "A-101".into(),
            date_time: "2026-10-19 05:30".into(),
            device: "DWG To PDF".into(),
            paper: "ARCH D".into(),
            scale: "1:48".into(),
        }
    }

    #[test]
    fn lines_follow_the_chosen_fields() {
        let s = PlotStamp::default();
        assert_eq!(s.lines(&facts()), ["Plan.dwg, A-101, 2026-10-19 05:30, DWG To PDF"]);
        let s =
            PlotStamp { paper_size: true, plot_scale: true, device_name: false, user_fields: vec!["Issued for bid".into()], ..PlotStamp::default() };
        assert_eq!(s.lines(&facts()), ["Plan.dwg, A-101, 2026-10-19 05:30, ARCH D, 1:48, Issued for bid"]);
        let two = PlotStamp { single_line: false, ..s };
        assert_eq!(two.lines(&facts()), ["Plan.dwg, A-101, 2026-10-19 05:30, ARCH D, 1:48", "Issued for bid"]);
        let none = PlotStamp { drawing_name: false, layout_name: false, date_time: false, device_name: false, ..PlotStamp::default() };
        assert!(none.lines(&facts()).is_empty());
    }

    #[test]
    fn hostile_settings_are_made_safe() {
        let s = PlotStamp {
            font: "Wingdings".into(),
            height_mm: f64::NAN,
            offset_mm: [f64::INFINITY, -1e9],
            user_fields: vec!["a\u{0}b\nc".into(), "x".repeat(1000), "  ".into()],
            ..PlotStamp::default()
        }
        .sanitized();
        assert_eq!((s.font.as_str(), s.height_mm), ("Helvetica", 2.0));
        assert_eq!(s.offset_mm, [0.0, -MAX_STAMP_OFFSET_MM]);
        assert_eq!(s.user_fields, vec!["abc".to_string(), "x".repeat(MAX_FIELD_CHARS)]);
        assert_eq!(PlotStamp { font: "courier".into(), ..PlotStamp::default() }.sanitized().font, "Courier");
    }
}
