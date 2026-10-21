//! New annotation too small to read: a dimension, TEXT or MTEXT whose characters are a speck at
//! the drawing's size (the classic first-day surprise: DIMSCALE 1 dimensions in a floor plan
//! drawn in inches). The command line says so once per drawing and kind, with a scale to try.

use cadaddy_doc::{EntityKind, Handle};

use crate::Session;

/// Text smaller than this fraction of the drawing's size is unreadable however it is plotted: on
/// an A1 sheet it is under a millimetre high.
const UNREADABLE: f64 = 1000.0;
/// The suggested size: about a hundredth of the drawing, a comfortable note or dimension height.
const COMFORTABLE: f64 = 100.0;

const IMPERIAL: [f64; 17] = [1.0, 2.0, 4.0, 8.0, 12.0, 16.0, 24.0, 32.0, 48.0, 64.0, 96.0, 128.0, 192.0, 240.0, 384.0, 480.0, 960.0];
const METRIC: [f64; 17] = [1.0, 2.0, 5.0, 10.0, 20.0, 25.0, 50.0, 100.0, 200.0, 250.0, 500.0, 1000.0, 2000.0, 2500.0, 5000.0, 10000.0, 20000.0];

/// Which kinds of annotation were already hinted about in a drawing (bits).
pub(crate) const DIM: u8 = 1;
pub(crate) const TEXT: u8 = 2;

/// What the check found: the annotation's height, the drawing's size and the scale to try.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TooSmall {
    pub height: f64,
    pub size: f64,
    pub scale: f64,
}

/// `height` (drawing units) is unreadably small in a drawing `size` across: the standard scale
/// that would make it comfortable. `imperial` picks from architectural scales (12, 24, 48…),
/// otherwise from metric ones (10, 20, 50…).
pub fn too_small(height: f64, size: f64, imperial: bool) -> Option<TooSmall> {
    if !(height > 0.0 && height.is_finite() && size.is_finite()) || size / height <= UNREADABLE {
        return None;
    }
    let want = size / COMFORTABLE / height;
    let scales: &[f64] = if imperial { &IMPERIAL } else { &METRIC };
    let scale = scales.iter().copied().min_by(|a, b| (a / want).ln().abs().total_cmp(&(b / want).ln().abs())).unwrap_or(1.0);
    Some(TooSmall { height, size, scale })
}

/// A round text height near `v`: 1, 1.5, 2, 3, 4, 6, 9 inches, 1', 1'-6"… in inches, else
/// 1, 2, 2.5, 3.5, 5, 7 times a power of ten (the pen sizes).
pub fn nice_height(v: f64, imperial: bool) -> f64 {
    if !(v.is_finite() && v > 0.0) {
        return 1.0;
    }
    let steps: &[f64] = if imperial { &[1.0, 1.5, 2.0, 3.0, 4.0, 6.0, 9.0] } else { &[1.0, 2.0, 2.5, 3.5, 5.0, 7.0] };
    let base = if imperial { 12.0_f64 } else { 10.0 };
    let decade = base.powf((v.ln() / base.ln()).floor().clamp(-12.0, 12.0));
    [decade / base, decade, decade * base]
        .into_iter()
        .flat_map(|d| steps.iter().map(move |s| s * d))
        .min_by(|a, b| (a / v).ln().abs().total_cmp(&(b / v).ln().abs()))
        .unwrap_or(v)
}

/// The height a new dimension or text object shows its characters at, and which kind it is;
/// `None` for other objects and for annotative ones (the annotation scale sizes those).
fn shown_height(s: &Session, h: Handle) -> Option<(f64, u8)> {
    let d = s.doc().ok()?;
    match &d.entity(h)?.kind {
        EntityKind::Dimension(dm) => {
            let st = d.dim_style(&dm.style)?.with_overrides(&dm.overrides);
            if st.annotative {
                return None;
            }
            Some((st.text_height * st.effective_scale(d.header.f64("DIMSCALE", 1.0)), DIM))
        }
        EntityKind::Text(t) => (!d.text_style(&t.style).is_some_and(|ts| ts.annotative)).then_some((t.height, TEXT)),
        EntityKind::MText(t) => (!d.text_style(&t.style).is_some_and(|ts| ts.annotative)).then_some((t.height, TEXT)),
        _ => None,
    }
}

/// Check a dimension or text object just made by a command; the first unreadable one of its
/// kind in the drawing gets a hint on the command line.
pub(crate) fn check_new(s: &mut Session, h: Handle) {
    if let Some((height, kind)) = shown_height(s, h) {
        check_height(s, height, kind);
    }
}

/// Check the height text is about to be written at (TEXT and MTEXT say it as they start), so the
/// hint comes before the typing; nothing for an annotative text style.
pub(crate) fn check_text_style_height(s: &mut Session, style: &str, height: f64) {
    if s.doc().is_ok_and(|d| !d.text_style(style).is_some_and(|ts| ts.annotative)) {
        check_height(s, height, TEXT);
    }
}

fn check_height(s: &mut Session, height: f64, kind: u8) {
    let Ok(state) = s.state() else { return };
    if state.legibility_hinted & kind != 0 {
        return;
    }
    let d = &state.doc;
    let ext = d.extents(&s.space());
    if ext.is_empty() {
        return;
    }
    let size = ext.width().max(ext.height());
    let imperial = matches!(d.header.i64("LUNITS", 2), 3 | 4) || matches!(d.header.i64("INSUNITS", 0), 1 | 2);
    let Some(t) = too_small(height, size, imperial) else { return };
    let (hs, bigger) = (crate::units::dist(s, t.height), crate::units::dist(s, nice_height(t.size / COMFORTABLE, imperial)));
    let k = t.scale;
    if kind == DIM {
        s.echo(format!("Dimension text {hs} high is too small to read in this drawing. Type DIMSCALE {k} to make new dimensions {k} times bigger."));
    } else {
        s.echo(format!("Text {hs} high is too small to read in this drawing. Try a height of {bigger} (TEXTSIZE sets the default)."));
    }
    if let Ok(st) = s.state_mut() {
        st.legibility_hinted |= kind;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_specks_are_too_small() {
        // DIMSCALE 1 text (0.18) in a 30-foot plan drawn in inches: 2000 times smaller.
        let t = too_small(0.18, 360.0, true).expect("too small");
        assert_eq!(t.scale, 24.0, "about a hundredth of the plan: 4.32\" text");
        // A 9" room label in the same plan, a 2.5 mm note on a 200 mm part, 1:100 text on a
        // 40 m building: all fine.
        assert_eq!(too_small(9.0, 360.0, true), None);
        assert_eq!(too_small(2.5, 200.0, false), None);
        assert_eq!(too_small(250.0, 40000.0, false), None);
        // 2.5 mm text on a 40 m building in millimetres: suggests 1:200 (500 mm text, a
        // hundredth of 40 m is 400 mm; 200 is the nearer standard scale to 160).
        let m = too_small(2.5, 40000.0, false).expect("too small");
        assert_eq!(m.scale, 200.0);
        // Round heights: 3.6" -> 4", 400 mm -> 350 mm, 1/8 of an inch stays.
        assert_eq!(nice_height(3.6, true), 4.0);
        assert_eq!(nice_height(400.0, false), 350.0);
        assert_eq!(nice_height(0.125, true), 0.125);
        assert_eq!(nice_height(f64::NAN, true), 1.0);
        // Hostile numbers never hint.
        assert_eq!(too_small(f64::NAN, 360.0, true), None);
        for (h, sz) in [(0.0, 1.0), (-1.0, 1e9), (f64::NAN, 1e9), (1.0, f64::INFINITY), (f64::MIN_POSITIVE, f64::MAX)] {
            let r = too_small(h, sz, false);
            assert!(r.is_none_or(|t| t.scale.is_finite()), "{h} {sz}: {r:?}");
        }
    }
}

#[cfg(test)]
mod session_tests {
    use cadaddy_doc::Drawing;
    use serde_json::json;

    use crate::Session;

    /// A 30' x 20' plan drawn in inches, as someone else's file arrives: DIMSCALE 1, TEXTSIZE 0.2.
    fn plan() -> Session {
        let mut s = Session::new();
        let mut d = Drawing::new_imperial();
        d.header.set_i64("LUNITS", 4);
        s.open_drawing(d, "plan.dxf", None);
        s.execute("rectang", &json!({ "p1": [0, 0], "p2": [360, 240] })).unwrap();
        s
    }

    fn hints(s: &Session, start: &str) -> usize {
        s.log.iter().filter(|l| l.starts_with(start) && l.contains("too small to read")).count()
    }

    #[test]
    fn a_speck_of_a_dimension_says_which_dimscale_to_use_once() {
        let mut s = plan();
        s.cmdline("dimlinear 6,6 182,6 90,30").unwrap();
        let hint = s.log.iter().find(|l| l.starts_with("Dimension text 0'-0 3/16\" high")).cloned().unwrap_or_default();
        assert!(hint.contains("Type DIMSCALE 24"), "{hint}");
        // Once per drawing: the next dimension doesn't repeat it.
        s.cmdline("dimlinear 6,6 6,200 30,90").unwrap();
        assert_eq!(hints(&s, "Dimension text"), 1);
        // Taking the advice: new dimensions are readable and nothing more is said.
        let mut fresh = plan();
        fresh.cmdline("dimscale 24").unwrap();
        fresh.cmdline("dimlinear 6,6 182,6 90,30").unwrap();
        assert_eq!(hints(&fresh, "Dimension text"), 0, "{:?}", fresh.log);
    }

    #[test]
    fn a_speck_of_text_suggests_a_height() {
        let mut s = plan();
        s.cmdline("text 20,20 0.2 0 Verify sizes on site").unwrap();
        s.cmdline("").unwrap();
        let hint = s.log.iter().find(|l| l.starts_with("Text 0'-0 3/16\" high")).cloned().unwrap_or_default();
        assert!(hint.contains("Try a height of 0'-4\""), "a round height, about a hundredth of the plan: {hint}");
        // MTEXT counts as the same kind: told once.
        s.cmdline("mtext 20,60 120,90 Second note").unwrap();
        s.cmdline("").unwrap();
        assert_eq!(hints(&s, "Text "), 1, "{:?}", s.log);
        // MTEXT says it as it starts, before anything is typed.
        let mut early = plan();
        early.cmdline("mtext").unwrap();
        assert!(early.running.is_some());
        assert_eq!(hints(&early, "Text "), 1, "{:?}", early.log);
        // Readable text in the same plan: no hint.
        let mut ok = plan();
        ok.cmdline("textsize 6").unwrap();
        ok.cmdline("text 20,20 6 0 ROOM").unwrap();
        ok.cmdline("").unwrap();
        assert_eq!(hints(&ok, "Text "), 0);
    }

    #[test]
    fn readable_and_annotative_annotation_is_left_alone() {
        // A small drawing: the default 0.18 dimension text is fine at 20 units across.
        let mut s = Session::new();
        s.execute("rectang", &json!({ "p1": [0, 0], "p2": [20, 10] })).unwrap();
        s.cmdline("dimlinear 0,0 20,0 10,-2").unwrap();
        assert_eq!(hints(&s, "Dimension text"), 0);
        // The floor-plan sample comes with DIMSCALE 24 and readable notes.
        let mut fp = Session::new();
        fp.open_drawing(crate::sample::floor_plan(), "floorplan", None);
        fp.cmdline("dimlinear 6,6 174,6 90,30").unwrap();
        fp.cmdline("mtext 20,60 120,90 Note").unwrap();
        fp.cmdline("").unwrap();
        assert_eq!(hints(&fp, "Dimension text") + hints(&fp, "Text "), 0, "{:?}", fp.log);
        // Annotative styles are sized by the annotation scale, not checked.
        let mut an = plan();
        for st in &mut an.doc_mut().unwrap().dim_styles {
            st.annotative = true;
        }
        an.cmdline("dimlinear 6,6 182,6 90,30").unwrap();
        assert_eq!(hints(&an, "Dimension text"), 0);
    }
}
