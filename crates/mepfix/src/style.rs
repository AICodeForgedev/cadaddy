//! How a fixture set is drafted, varied by seed: abbreviations, how sizes and heights are written,
//! line weights, symbols, scales, paper and title block layout. Readers must cope with all of
//! these, as they must with real offices' different standards.

use crate::rng::Rng;
use crate::truth::{feet_inches, fmt_in, pipe_size};

#[derive(Clone, Debug)]
pub struct Style {
    /// Printed abbreviation per system code.
    pub abbr: Vec<(&'static str, &'static str)>,
    /// Rectangular size: `24x14`, `24"x14"`, `24/14`.
    pub rect: RectTag,
    /// Round size: `12"Ø`, `12" DIA`, `12Ø`, `12" RD`.
    pub round: RoundTag,
    /// Height notes: which reference and how it is written.
    pub height: HeightNote,
    /// Size tag joined to its run by a leader (otherwise placed beside it).
    pub leaders: bool,
    /// Duct centrelines drawn.
    pub centrelines: bool,
    /// Radius elbows (otherwise mitred corners).
    pub radius_elbows: bool,
    /// Tags turned along their run (otherwise always horizontal).
    pub tags_along: bool,
    /// Share of horizontal duct runs carrying a height note (mains always do).
    pub noted_share: f64,
    /// Pen widths, mm: duct/pipe outline, background (walls), text, title block frame.
    pub lw_run: u16,
    pub lw_background: u16,
    pub lw_text: u16,
    pub lw_frame: u16,
    /// Paper, inches (landscape).
    pub paper: (&'static str, [f64; 2]),
    /// Title block down the right side (otherwise along the bottom).
    pub title_right: bool,
    /// Grid: numbers on the lines of constant x (otherwise letters there).
    pub numbers_on_x: bool,
    /// Riser symbols: 0 = circle with a dot (up) / circle with a half fill (down), 1 = circle with
    /// a cross (up) / circle with a line (down).
    pub riser_symbols: u8,
    /// Text height on paper, inches.
    pub text_in: f64,
    /// Ceiling tags: `CLG @ 9'-0"` + type, or `9'-0" AFF` + type, or a type/height bubble.
    pub ceiling_tag: u8,
    /// Sprinkler heads: 0 circle with a cross, 1 filled dot in a circle.
    pub head_symbol: u8,
    /// Drainage slope printed as `1/4"/FT` or `1/4" PER FT` or `2%`.
    pub slope: u8,
    /// Every sheet's grid origin starts somewhere else in the drawing's coordinates.
    pub world_offsets: bool,
    /// The PDFs keep the drawing's layers (optional content); most printed sets don't.
    pub pdf_layers: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RectTag {
    #[default]
    Plain,
    Inches,
    Slash,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RoundTag {
    #[default]
    Phi,
    Dia,
    PhiBare,
    Rd,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HeightNote {
    /// `BOD @ 10'-6"`
    #[default]
    BodAt,
    /// `B.O.D. 10'-6" AFF`
    BodAff,
    /// `BOT EL 10'-6"`
    BotEl,
    /// `TOD @ 11'-8"` (top of duct)
    TodAt,
    /// `CL @ 11'-0"`
    ClAt,
}

pub const PAPERS: &[(&str, [f64; 2])] = &[("ARCH D", [36.0, 24.0]), ("ANSI D", [34.0, 22.0]), ("ARCH E1", [42.0, 30.0])];

impl Style {
    pub fn new(r: &mut Rng) -> Style {
        let supply = r.pick(&["SA", "S", "SUP"]);
        let ret = r.pick(&["RA", "R", "RET"]);
        let exhaust = r.pick(&["EA", "EX", "E"]);
        let outside = r.pick(&["OA", "OSA"]);
        let hwr = r.pick(&["HWR", "HWC", "HRW"]);
        let gas = r.pick(&["G", "GAS", "NG"]);
        let san = r.pick(&["SAN", "SS", "W"]);
        Style {
            abbr: vec![
                ("SA", supply),
                ("RA", ret),
                ("EA", exhaust),
                ("OA", outside),
                ("CW", "CW"),
                ("HW", "HW"),
                ("HWR", hwr),
                ("SAN", san),
                ("V", "V"),
                ("G", gas),
                ("FP", r.pick(&["FP", "F", "SP"])),
            ],
            rect: r.pick(&[RectTag::Plain, RectTag::Plain, RectTag::Inches, RectTag::Slash]),
            round: r.pick(&[RoundTag::Phi, RoundTag::Dia, RoundTag::PhiBare, RoundTag::Rd]),
            height: r.pick(&[HeightNote::BodAt, HeightNote::BodAt, HeightNote::BodAff, HeightNote::BotEl, HeightNote::TodAt, HeightNote::ClAt]),
            leaders: r.chance(0.4),
            centrelines: r.chance(0.6),
            radius_elbows: r.chance(0.6),
            tags_along: r.chance(0.5),
            noted_share: r.pick::<f64>(&[0.3, 0.5, 0.8, 1.0]),
            lw_run: r.pick(&[35, 40, 50]),
            lw_background: r.pick(&[13, 18]),
            lw_text: r.pick(&[18, 25]),
            lw_frame: r.pick(&[50, 70]),
            paper: r.pick(PAPERS),
            title_right: r.chance(0.7),
            numbers_on_x: r.chance(0.7),
            riser_symbols: r.pick(&[0, 1]),
            text_in: r.pick::<f64>(&[0.09375, 0.1, 0.125]),
            ceiling_tag: r.pick(&[0, 1, 2]),
            head_symbol: r.pick(&[0, 1]),
            slope: r.pick(&[0, 1, 2]),
            world_offsets: r.chance(0.5),
            pdf_layers: r.chance(0.3),
        }
    }

    /// The printed abbreviation of a system code.
    pub fn sys(&self, code: &str) -> String {
        self.abbr.iter().find(|(c, _)| *c == code).map(|(_, a)| (*a).to_string()).unwrap_or_else(|| code.to_string())
    }

    pub fn rect_size(&self, w: f64, h: f64) -> String {
        match self.rect {
            RectTag::Plain => format!("{}x{}", fmt_in(w), fmt_in(h)),
            RectTag::Inches => format!("{}\"x{}\"", fmt_in(w), fmt_in(h)),
            RectTag::Slash => format!("{}/{}", fmt_in(w), fmt_in(h)),
        }
    }

    pub fn round_size(&self, d: f64) -> String {
        match self.round {
            RoundTag::Phi => format!("{}\"Ø", fmt_in(d)),
            RoundTag::Dia => format!("{}\" DIA", fmt_in(d)),
            RoundTag::PhiBare => format!("{}Ø", fmt_in(d)),
            RoundTag::Rd => format!("{}\" RD", fmt_in(d)),
        }
    }

    pub fn pipe_size(&self, d: f64) -> String {
        format!("{}\"", pipe_size(d))
    }

    /// The height note for a duct whose bottom, top and centreline are given (feet above the
    /// floor).
    pub fn height_note(&self, bottom: f64, top: f64, centre: f64) -> String {
        match self.height {
            HeightNote::BodAt => format!("BOD @ {}", feet_inches(bottom)),
            HeightNote::BodAff => format!("B.O.D. {} AFF", feet_inches(bottom)),
            HeightNote::BotEl => format!("BOT EL {}", feet_inches(bottom)),
            HeightNote::TodAt => format!("TOD @ {}", feet_inches(top)),
            HeightNote::ClAt => format!("CL @ {}", feet_inches(centre)),
        }
    }

    /// The height note for a pipe (bottom of pipe or centreline).
    pub fn pipe_height_note(&self, bottom: f64, centre: f64) -> String {
        match self.height {
            HeightNote::ClAt | HeightNote::TodAt => format!("CL @ {}", feet_inches(centre)),
            HeightNote::BodAff => format!("B.O.P. {} AFF", feet_inches(bottom)),
            _ => format!("BOP @ {}", feet_inches(bottom)),
        }
    }

    /// A drainage slope, inches per foot.
    pub fn slope_text(&self, in_per_ft: f64) -> String {
        let t = if (in_per_ft - 0.25).abs() < 1e-9 { "1/4" } else { "1/8" };
        match self.slope {
            0 => format!("{t}\"/FT"),
            1 => format!("{t}\" PER FT"),
            _ => format!("{}%", if t == "1/4" { "2" } else { "1" }),
        }
    }

    /// A ceiling tag's lines.
    pub fn ceiling_text(&self, height_ft: f64, kind: &str) -> Vec<String> {
        match self.ceiling_tag {
            0 => vec![format!("CLG @ {}", feet_inches(height_ft)), kind.to_string()],
            1 => vec![format!("{} AFF", feet_inches(height_ft)), kind.to_string()],
            _ => vec![kind.to_string(), feet_inches(height_ft)],
        }
    }
}

/// Plan scales: printed text and paper inches per foot.
pub const SCALES: &[(&str, f64)] = &[("1/8\" = 1'-0\"", 0.125), ("3/16\" = 1'-0\"", 0.1875), ("1/4\" = 1'-0\"", 0.25)];
