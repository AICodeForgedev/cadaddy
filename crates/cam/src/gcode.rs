//! G-code generation from a [`ToolpathResult`].
//!
//! Three dialects are supported:
//! - **Generic** (ISO 6983): the baseline that most controllers understand.
//! - **LinuxCNC** (also Grbl-compatible): adds `G64 P…` path blending.
//! - **Mach3** (Mach3 / Mach4): explicit `G40` cutter-comp cancel.
//!
//! All dialects emit the same body; only the header and footer differ.

use std::fmt::Write as FmtWrite;

use serde::{Deserialize, Serialize};

use crate::toolpath::{Move, ToolpathResult};
use crate::{CamJob, Result};

/// G-code output dialect.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Dialect {
    /// ISO 6983 standard G-code.
    #[default]
    Generic,
    /// LinuxCNC / Grbl.
    #[serde(alias = "grbl")]
    LinuxCnc,
    /// Mach3 / Mach4.
    Mach3,
}

/// Options for G-code formatting.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GcodeOptions {
    pub dialect: Dialect,
    /// Whether to use metric (G21) or imperial (G20).  Default: metric.
    pub metric: bool,
    /// Number of decimal places for coordinates.  Default: 4.
    pub decimals: usize,
    /// Path-blending tolerance for LinuxCNC (mm or in).  Default: 0.01.
    pub blend_tolerance: f64,
    /// A program number for the `O` line (0 = omit).
    pub program_number: u32,
}

impl Default for GcodeOptions {
    fn default() -> Self {
        Self { dialect: Dialect::Generic, metric: true, decimals: 4, blend_tolerance: 0.01, program_number: 0 }
    }
}

/// Emit G-code text from a toolpath result.
pub fn emit(result: &ToolpathResult, job: &CamJob, opts: &GcodeOptions) -> Result<String> {
    let mut out = String::with_capacity(4096);
    let d = opts.decimals;

    // Header.
    emit_header(&mut out, job, opts)?;

    // Body: each profile's moves.
    for (pi, path) in result.paths.iter().enumerate() {
        let _ = writeln!(out, "(Profile {pi}: source index {})", path.profile_index);
        for m in &path.moves {
            emit_move(&mut out, m, d)?;
        }
    }

    // Footer.
    emit_footer(&mut out, opts)?;

    Ok(out)
}

/// Parse G-code back into moves (for testing round-trip correctness).
pub fn parse(text: &str) -> Vec<Move> {
    let mut moves = Vec::new();
    let mut cur_f = 0.0_f64;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('(') || line.starts_with('%') || line.starts_with(';') {
            continue;
        }
        let upper = line.to_ascii_uppercase();
        if let Some(m) = parse_move(&upper, &mut cur_f) {
            moves.push(m);
        }
    }
    moves
}

// ── Header / footer ──────────────────────────────────────────────────────────

fn emit_header(out: &mut String, job: &CamJob, opts: &GcodeOptions) -> Result<()> {
    let _ = writeln!(out, "%");
    if opts.program_number > 0 {
        let _ = writeln!(out, "O{:04}", opts.program_number);
    }
    let _ = writeln!(out, "(CADaddy CAM 2D Profile)");
    let _ = writeln!(out, "(Tool diameter: {:.4})", job.tool.diameter);

    // Units.
    if opts.metric {
        let _ = writeln!(out, "G21");
    } else {
        let _ = writeln!(out, "G20");
    }
    // Absolute coordinates, XY plane.
    let _ = writeln!(out, "G90");
    let _ = writeln!(out, "G17");

    match opts.dialect {
        Dialect::Generic => {}
        Dialect::LinuxCnc => {
            let _ = writeln!(out, "G64 P{:.4}", opts.blend_tolerance);
        }
        Dialect::Mach3 => {
            let _ = writeln!(out, "G40"); // cancel cutter comp
        }
    }

    // Spindle on.
    let _ = writeln!(out, "S{:.0} M03", job.tool.spindle_speed);
    Ok(())
}

fn emit_footer(out: &mut String, opts: &GcodeOptions) -> Result<()> {
    let _ = writeln!(out, "M05"); // spindle off
    match opts.dialect {
        Dialect::Generic | Dialect::LinuxCnc => {}
        Dialect::Mach3 => {
            let _ = writeln!(out, "G40"); // cancel cutter comp
        }
    }
    let _ = writeln!(out, "M30"); // program end + rewind
    let _ = writeln!(out, "%");
    Ok(())
}

// ── Move emission ────────────────────────────────────────────────────────────

fn emit_move(out: &mut String, m: &Move, d: usize) -> Result<()> {
    match *m {
        Move::Rapid { x, y, z } => {
            let _ = writeln!(out, "G00 X{x:.d$} Y{y:.d$} Z{z:.d$}");
        }
        Move::Feed { x, y, z, f } => {
            let _ = writeln!(out, "G01 X{x:.d$} Y{y:.d$} Z{z:.d$} F{f:.1}");
        }
        Move::ArcCw { x, y, z, i, j, f } => {
            let _ = writeln!(out, "G02 X{x:.d$} Y{y:.d$} Z{z:.d$} I{i:.d$} J{j:.d$} F{f:.1}");
        }
        Move::ArcCcw { x, y, z, i, j, f } => {
            let _ = writeln!(out, "G03 X{x:.d$} Y{y:.d$} Z{z:.d$} I{i:.d$} J{j:.d$} F{f:.1}");
        }
    }
    Ok(())
}

// ── G-code parsing (for test round-trips) ────────────────────────────────────

fn parse_move(line: &str, cur_f: &mut f64) -> Option<Move> {
    let code = extract_word(line, 'G')?;
    let x = extract_word(line, 'X').unwrap_or(0.0);
    let y = extract_word(line, 'Y').unwrap_or(0.0);
    let z = extract_word(line, 'Z').unwrap_or(0.0);
    if let Some(f) = extract_word(line, 'F') {
        *cur_f = f;
    }
    let f = *cur_f;
    let code_int = code as i64;
    match code_int {
        0 => Some(Move::Rapid { x, y, z }),
        1 => Some(Move::Feed { x, y, z, f }),
        2 => {
            let i = extract_word(line, 'I').unwrap_or(0.0);
            let j = extract_word(line, 'J').unwrap_or(0.0);
            Some(Move::ArcCw { x, y, z, i, j, f })
        }
        3 => {
            let i = extract_word(line, 'I').unwrap_or(0.0);
            let j = extract_word(line, 'J').unwrap_or(0.0);
            Some(Move::ArcCcw { x, y, z, i, j, f })
        }
        _ => None,
    }
}

fn extract_word(line: &str, letter: char) -> Option<f64> {
    let pos = line.find(letter)?;
    let rest = &line[pos + 1..];
    // Take characters that are part of a number.
    let num_str: String = rest.chars().take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-' || *c == '+').collect();
    num_str.parse::<f64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::toolpath::{Move, compute};
    use crate::{CamJob, LeadKind, LeadParams, OffsetSide, TabParams, ToolParams};
    use cadaddy_geom::{PolyVertex, Polyline, Vec2};

    fn square() -> Polyline {
        Polyline {
            vertices: vec![
                PolyVertex::new(Vec2::new(0.0, 0.0)),
                PolyVertex::new(Vec2::new(10.0, 0.0)),
                PolyVertex::new(Vec2::new(10.0, 10.0)),
                PolyVertex::new(Vec2::new(0.0, 10.0)),
            ],
            closed: true,
        }
    }

    fn basic_job() -> CamJob {
        CamJob {
            tool: ToolParams {
                diameter: 6.0,
                feed_rate: 500.0,
                plunge_rate: 100.0,
                spindle_speed: 12000.0,
                depth: 3.0,
                safe_z: 10.0,
                step_down: 3.0,
            },
            offset: OffsetSide::On,
            lead_in: LeadParams { kind: LeadKind::None, length: 0.0 },
            lead_out: LeadParams { kind: LeadKind::None, length: 0.0 },
            tabs: TabParams { count: 0, ..TabParams::default() },
            order: crate::CutOrder::Nearest,
        }
    }

    #[test]
    fn generic_gcode_round_trips() {
        let tp = compute(&[square()], &basic_job());
        assert!(tp.is_ok());
        let tp = tp.ok();
        let gcode = emit(tp.as_ref().unwrap_or_else(|| std::process::abort()), &basic_job(), &GcodeOptions::default());
        assert!(gcode.is_ok());
        let text = gcode.ok().unwrap_or_default();
        assert!(text.contains("G21"), "should have metric");
        assert!(text.contains("G90"), "should have absolute");
        assert!(text.contains("M03"), "should have spindle on");
        assert!(text.contains("M30"), "should have program end");

        // Parse back and verify move count matches.
        let parsed = parse(&text);
        let original_moves: Vec<&Move> = tp.iter().flat_map(|t| t.paths.iter().flat_map(|p| &p.moves)).collect();
        assert_eq!(parsed.len(), original_moves.len(), "parsed {}, original {}", parsed.len(), original_moves.len());
    }

    #[test]
    fn linuxcnc_has_blend() {
        let tp = compute(&[square()], &basic_job());
        let tp_ref = tp.as_ref().ok();
        if let Some(tp) = tp_ref {
            let opts = GcodeOptions { dialect: Dialect::LinuxCnc, ..Default::default() };
            let text = emit(tp, &basic_job(), &opts).unwrap_or_default();
            assert!(text.contains("G64 P"), "LinuxCNC should have path blending");
        }
    }

    #[test]
    fn mach3_has_g40() {
        let tp = compute(&[square()], &basic_job());
        let tp_ref = tp.as_ref().ok();
        if let Some(tp) = tp_ref {
            let opts = GcodeOptions { dialect: Dialect::Mach3, ..Default::default() };
            let text = emit(tp, &basic_job(), &opts).unwrap_or_default();
            assert!(text.contains("G40"), "Mach3 should have cutter comp cancel");
        }
    }

    #[test]
    fn parse_extracts_arcs() {
        let line = "G02 X10.0000 Y0.0000 Z-3.0000 I5.0000 J0.0000 F500.0";
        let mut f = 0.0;
        let m = parse_move(&line.to_ascii_uppercase(), &mut f);
        assert!(matches!(m, Some(Move::ArcCw { .. })));
        if let Some(Move::ArcCw { x, i, f, .. }) = m {
            assert!((x - 10.0).abs() < 1e-6);
            assert!((i - 5.0).abs() < 1e-6);
            assert!((f - 500.0).abs() < 1e-6);
        }
    }

    #[test]
    fn empty_toolpath_still_has_header_footer() {
        // Manually construct an empty toolpath result.
        let result = crate::toolpath::ToolpathResult { paths: Vec::new(), offset_profiles: Vec::new() };
        let text = emit(&result, &basic_job(), &GcodeOptions::default()).unwrap_or_default();
        assert!(text.contains("M03"));
        assert!(text.contains("M30"));
    }
}
