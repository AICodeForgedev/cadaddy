//! ARCTEXT: text along an arc. CADaddy doesn't have an arc-aligned text object: each character
//! is a TEXT turned to the arc, and the characters are kept together as an unnamed group (select
//! one, the group comes along; UNGROUP or EXPLODE frees them).
//!
//! Convex text sits outside the arc and reads clockwise (over the top of an arch); concave text
//! sits inside and reads counterclockwise (along the bottom of a bowl). Fit spreads the
//! characters over the whole arc; Left, Right and Center keep their natural spacing.

use cadaddy_doc::{EntityKind, HAlign, Handle, Text, VAlign};
use cadaddy_geom::{Vec2, Vec3};
use serde_json::{Value, json};

use super::super::*;
use crate::{Accept, Input, Interactive, Prompt, Result, Session, Step};

/// Most characters one arc text holds.
const MAX_CHARS: usize = 2000;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Align {
    Fit,
    Left,
    Right,
    Center,
}

/// Where each character goes: its middle point and rotation (radians).
#[allow(clippy::too_many_arguments)]
fn layout(center: Vec2, radius: f64, start: f64, end: f64, widths: &[f64], align: Align, convex: bool, gap: f64) -> Result<Vec<(Vec2, f64)>> {
    let mut sweep = end - start;
    while sweep <= 0.0 {
        sweep += std::f64::consts::TAU;
    }
    let n = widths.len();
    let natural: f64 = widths.iter().sum::<f64>() + gap * n.saturating_sub(1) as f64;
    let arc_len = radius * sweep;
    let (lead, extra) = match align {
        Align::Fit if n > 1 => (0.0, (arc_len - widths.iter().sum::<f64>()) / (n - 1) as f64),
        Align::Fit => ((arc_len - natural) / 2.0, gap),
        Align::Left => (0.0, gap),
        Align::Right => (arc_len - natural, gap),
        Align::Center => ((arc_len - natural) / 2.0, gap),
    };
    if align == Align::Fit && extra < 0.0 || natural > arc_len + 1e-9 && align != Align::Fit {
        return Err(other("The text is longer than the arc; use a smaller height or a longer arc."));
    }
    let mut out = Vec::with_capacity(n);
    let mut along = lead;
    for w in widths {
        let mid = (along + w / 2.0) / radius;
        // Convex: from the arc's end, clockwise. Concave: from its start, counterclockwise.
        let a = if convex { end - mid } else { start + mid };
        let at = center + Vec2::from_angle(a) * radius;
        let rot = if convex { a - std::f64::consts::FRAC_PI_2 } else { a + std::f64::consts::FRAC_PI_2 };
        out.push((at, rot));
        along += w + extra;
    }
    Ok(out)
}

pub(crate) fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let arc_h = super::super::curves::handle_param(p, "arc").ok_or_else(|| bad("arctext", "`arc` (handle) is required"))?;
    let text = p.get("text").and_then(Value::as_str).ok_or_else(|| bad("arctext", "`text` is required"))?;
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() || chars.len() > MAX_CHARS {
        return Err(bad("arctext", format!("`text` has 1 to {MAX_CHARS} characters")));
    }
    let d = s.doc()?;
    let EntityKind::Arc(arc) = &d.entity(arc_h).ok_or_else(|| other("No such arc."))?.kind else {
        return Err(other("Select an arc."));
    };
    let arc = arc.clone();
    let style = p.get("style").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| d.header.str("TEXTSTYLE", "Standard"));
    let ts = d.text_style(&style).cloned().ok_or_else(|| other(format!("No text style named \"{style}\".")))?;
    let height = p.get("height").and_then(Value::as_f64).unwrap_or(if ts.height > 0.0 { ts.height } else { d.header.f64("TEXTSIZE", 0.2) });
    let wf = p.get("widthFactor").and_then(Value::as_f64).unwrap_or(if ts.width_factor > 0.0 { ts.width_factor } else { 1.0 });
    let offset = p.get("offset").and_then(Value::as_f64).unwrap_or(0.0);
    let gap = p.get("spacing").and_then(Value::as_f64).unwrap_or(0.0);
    if ![height, wf, offset, gap].iter().all(|v| v.is_finite()) || height <= 0.0 || wf <= 0.0 || height > 1e9 {
        return Err(bad("arctext", "height, widthFactor, offset and spacing must be finite (height and widthFactor above 0)"));
    }
    let convex = match p.get("side").and_then(Value::as_str).map(str::to_ascii_lowercase).as_deref() {
        None | Some("convex") => true,
        Some("concave") => false,
        Some(_) => return Err(bad("arctext", "`side` is \"convex\" or \"concave\"")),
    };
    let align = match p.get("align").and_then(Value::as_str).map(str::to_ascii_lowercase).as_deref() {
        None | Some("fit") => Align::Fit,
        Some("left") => Align::Left,
        Some("right") => Align::Right,
        Some("center") => Align::Center,
        Some(_) => return Err(bad("arctext", "`align` is \"fit\", \"left\", \"right\" or \"center\"")),
    };
    // Convex text stands on a circle `offset` outside the arc; concave text hangs `offset` inside.
    let radius = if convex { arc.radius + offset } else { arc.radius - offset };
    if !(radius.is_finite() && radius > 1e-9) {
        return Err(other("The offset puts the text past the arc's centre."));
    }
    let font = cadaddy_fonts::TextFont::resolve(&ts.font);
    let widths: Vec<f64> = chars
        .iter()
        .map(|c| {
            let w = cadaddy_fonts::text_width(&font, &c.to_string(), height, wf);
            if w > 0.0 { w } else { height * 0.5 * wf }
        })
        .collect();
    let places = layout(arc.center.xy(), radius, arc.start, arc.end, &widths, align, convex, gap)?;
    let z = arc.center.z;
    let mut made = Vec::new();
    for (c, (at, rot)) in chars.iter().zip(places) {
        if c.is_whitespace() {
            continue;
        }
        let t = Text {
            insert: Vec3::new(at.x, at.y, z),
            align_pt: Some(Vec3::new(at.x, at.y, z)),
            height,
            value: c.to_string(),
            rotation: rot,
            width_factor: wf,
            oblique: 0.0,
            style: style.clone(),
            halign: HAlign::Center,
            valign: VAlign::Baseline,
        };
        made.push(s.add_entity(EntityKind::Text(t))?);
    }
    if made.is_empty() {
        return Err(other("The text has only spaces."));
    }
    let group = super::super::groups::create(s, None, &format!("ARCTEXT {text}"), &made, true)?;
    Ok(json!({ "handles": made.iter().map(|h| h.hex()).collect::<Vec<_>>(), "group": group }))
}

#[derive(Default)]
pub(crate) struct ArctextM {
    arc: Option<Handle>,
}

impl Interactive for ArctextM {
    fn name(&self) -> &'static str {
        "ARCTEXT"
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        match self.arc {
            None => Prompt::new("Select an Arc", Accept::POINT),
            Some(_) => Prompt::new("Enter text", Accept::TEXT),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        match (self.arc, i) {
            (_, Input::Cancel) => Ok(Step::Cancel),
            (None, Input::Point(p)) => {
                let h = super::super::curves::pick_at(s, p).ok_or_else(|| other("*Invalid selection*"))?;
                if !matches!(super::super::curves::entity(s, h)?.kind, EntityKind::Arc(_)) {
                    return Err(other("Select an arc."));
                }
                self.arc = Some(h);
                Ok(Step::Continue)
            }
            (None, Input::Enter) => Ok(Step::Cancel),
            (Some(h), Input::Text(t)) if !t.trim().is_empty() => {
                run(s, &json!({ "arc": h.hex(), "text": t }))?;
                Ok(Step::Done)
            }
            (Some(_), Input::Enter) => Ok(Step::Cancel),
            _ => Ok(Step::Continue),
        }
    }
}

fn other(m: impl Into<String>) -> crate::EngineError {
    crate::EngineError::Other(m.into())
}
