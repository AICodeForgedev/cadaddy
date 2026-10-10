//! Point modifiers typed at a point prompt, as in AutoCAD: `FROM` (a base point and an offset),
//! `M2P`/`MTP` (the midpoint between two points), the point filters `.x`, `.y`, `.z`, `.xy`,
//! `.xz`, `.yz` and the angle override `<a`.
//!
//! A modifier pushes a [`Frame`] onto [`Session::point_mods`]. While frames are pending the
//! session shows their prompt ("Base point:", "<Offset>:", "First point of mid:", ".X of",
//! "(need YZ):") instead of the command's, collects the points they need and finally hands one
//! point to the command. Every point prompt of every command gets them this way, from the command
//! line and in scripts alike, and they nest (`FROM` then `M2P`).

use cadcraft_geom::Vec2;

use crate::prompt::{Accept, Input, Prompt};
use crate::{Result, Session};

/// The most modifiers pending at once (bounds hostile scripts such as `FROM FROM FROM …`).
const MAX_FRAMES: usize = 32;

/// The coordinates a point filter takes from its first point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Axes {
    pub x: bool,
    pub y: bool,
    pub z: bool,
}

impl Axes {
    /// `.x`, `.y`, `.z`, `.xy`, `.xz` or `.yz` (case-insensitive).
    pub fn parse(t: &str) -> Option<Axes> {
        let a = match t.to_ascii_lowercase().as_str() {
            ".x" => (true, false, false),
            ".y" => (false, true, false),
            ".z" => (false, false, true),
            ".xy" => (true, true, false),
            ".xz" => (true, false, true),
            ".yz" => (false, true, true),
            _ => return None,
        };
        Some(Axes { x: a.0, y: a.1, z: a.2 })
    }

    fn letters(self, keep: bool) -> String {
        [(self.x, 'X'), (self.y, 'Y'), (self.z, 'Z')].iter().filter(|(on, _)| *on == keep).map(|(_, c)| *c).collect()
    }

    /// The filter as typed: ".XY".
    pub fn name(self) -> String {
        format!(".{}", self.letters(true))
    }

    /// The coordinates still needed: "YZ".
    pub fn need(self) -> String {
        self.letters(false)
    }

    /// The filtered coordinates of `first` with the rest from `second` (Z is not stored: there
    /// is no 3D yet, so a Z taken from either point is dropped).
    pub fn combine(self, first: Vec2, second: Vec2) -> Vec2 {
        Vec2::new(if self.x { first.x } else { second.x }, if self.y { first.y } else { second.y })
    }
}

/// A pending point modifier.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Frame {
    /// `FROM`: the base point once given, then the offset point.
    From { base: Option<Vec2> },
    /// `M2P`/`MTP`: the first point once given, then the second.
    Mid { first: Option<Vec2> },
    /// A point filter: the point to take `keep` from once given, then the rest.
    Filter { keep: Axes, first: Option<Vec2> },
    /// `<a`: the next point lies on the line through `base` in direction `dir` (a unit vector).
    Angle { base: Vec2, dir: Vec2 },
}

/// Strip the international (`_`) and transparent (`'`) prefixes a modifier may be typed with.
fn bare(t: &str) -> &str {
    t.trim().trim_start_matches(['\'', '_'])
}

/// Whether `t` is exactly one of the prompt's keywords (the whole word or its capital-letter
/// shortcut): a command's own keyword wins over a modifier spelled the same.
fn exact_keyword(prompt: &Prompt, t: &str) -> bool {
    prompt.match_keyword(t).is_some_and(|k| {
        let short: String = k.chars().filter(|c| c.is_ascii_uppercase() || c.is_ascii_digit()).collect();
        k.eq_ignore_ascii_case(t) || short.eq_ignore_ascii_case(t)
    })
}

/// Whether point modifiers apply at this prompt: it asks for a point that is not an object pick.
pub fn accepts(prompt: &Prompt) -> bool {
    prompt.accept.point && !prompt.picks_objects()
}

impl Session {
    /// The prompt shown while modifiers are pending, built over the command's own prompt.
    pub(crate) fn overlay_prompt(&self, cmd: Prompt) -> Prompt {
        let point = Accept::POINT;
        self.point_mods.iter().fold(cmd, |p, f| match *f {
            Frame::From { base: None } => Prompt::new("Base point", point),
            Frame::From { base: Some(b) } => Prompt::new("<Offset>", point).base(b),
            Frame::Mid { first: None } => Prompt::new("First point of mid", point),
            Frame::Mid { first: Some(f) } => Prompt::new("Second point of mid", point).base(f),
            Frame::Filter { keep, first: None } => Prompt::new(format!("{} of", keep.name()), point).base_opt(p.base),
            Frame::Filter { keep, first: Some(_) } => Prompt::new(format!("(need {})", keep.need()), point).base_opt(p.base),
            Frame::Angle { .. } => p,
        })
    }

    /// The modifier `t` names at `prompt`, if any.
    fn parse_modifier(&self, prompt: &Prompt, t: &str) -> Option<Frame> {
        if exact_keyword(prompt, t) {
            return None;
        }
        let b = bare(t);
        if let Some(a) = b.strip_prefix('<') {
            // `<a`, `<<a`, `<<<a`, read like the angle of `@d<a`.
            let angles = self.angle_settings();
            let ang = if a.starts_with('<') { angles.direction(&format!("<{a}"))? } else { angles.direction(a)? };
            let dir = Vec2::from_angle(ang);
            let base = prompt.base.unwrap_or(self.last_point);
            return (dir.is_finite() && base.is_finite()).then_some(Frame::Angle { base, dir });
        }
        if let Some(keep) = Axes::parse(b) {
            return Some(Frame::Filter { keep, first: None });
        }
        match b.to_ascii_lowercase().as_str() {
            "from" | "fro" => Some(Frame::From { base: None }),
            "m2p" | "mtp" => Some(Frame::Mid { first: None }),
            _ => None,
        }
    }

    /// A token typed at a point prompt that a modifier handles: a modifier name, or a distance
    /// along an angle override. `None` leaves the token to the usual parsing.
    pub(crate) fn typed_modifier(&mut self, prompt: &Prompt, t: &str) -> Option<Result<()>> {
        if !accepts(prompt) {
            return None;
        }
        if let Some(f) = self.parse_modifier(prompt, t) {
            if self.point_mods.len() >= MAX_FRAMES {
                self.echo("Too many point modifiers.");
                return Some(Ok(()));
            }
            if let Frame::Angle { .. } = f {
                self.echo(format!("Angle Override: {}", bare(t).trim_start_matches('<')));
            }
            self.point_mods.push(f);
            return Some(Ok(()));
        }
        if let Some(Frame::Angle { base, dir }) = self.point_mods.last().copied() {
            // Typed coordinates are explicit: they win over the override.
            if crate::prompt::parse_point_with(t, self.last_point, &self.angle_settings()).is_some() {
                self.point_mods.pop();
                return None;
            }
            if let Some(d) = crate::units::parse_distance(t) {
                self.point_mods.pop();
                return Some(self.modifier_point(base + dir * d));
            }
        }
        None
    }

    /// An input while modifiers are pending.
    pub(crate) fn modifier_input(&mut self, input: Input) -> Result<()> {
        // An angle override only constrains a point: anything else goes to the prompt below it.
        if let Some(Frame::Angle { .. }) = self.point_mods.last()
            && !matches!(input, Input::Point(_) | Input::Deferred(_))
        {
            self.point_mods.pop();
            return self.input(input);
        }
        match input {
            Input::Point(p) => self.modifier_point(p),
            Input::Deferred(d) => self.modifier_point(d.at),
            // Enter drops the modifiers and returns to the command's prompt.
            Input::Enter => {
                self.point_mods.clear();
                Ok(())
            }
            Input::Cancel => {
                self.point_mods.clear();
                self.input(Input::Cancel)
            }
            Input::Keyword(_) | Input::Text(_) | Input::Pick(_) => {
                self.echo("Invalid point.");
                Ok(())
            }
        }
    }

    /// A point given while modifiers are pending: complete the top frame, and when it has its
    /// point, hand that to the frame (or command) below.
    pub(crate) fn modifier_point(&mut self, mut p: Vec2) -> Result<()> {
        if !p.is_finite() {
            self.echo("Invalid point.");
            return Ok(());
        }
        while let Some(top) = self.point_mods.pop() {
            match top {
                Frame::From { base: None } => {
                    self.point_mods.push(Frame::From { base: Some(p) });
                    // `@dx,dy` at "<Offset>:" is measured from the base point.
                    self.last_point = p;
                    return Ok(());
                }
                Frame::Mid { first: None } => {
                    self.point_mods.push(Frame::Mid { first: Some(p) });
                    self.last_point = p;
                    return Ok(());
                }
                Frame::Filter { keep, first: None } => {
                    self.point_mods.push(Frame::Filter { keep, first: Some(p) });
                    return Ok(());
                }
                // The offset point is already measured from the base (`@` reads the last point).
                Frame::From { base: Some(_) } => {}
                Frame::Mid { first: Some(f) } => p = (f + p) * 0.5,
                Frame::Filter { keep, first: Some(f) } => p = keep.combine(f, p),
                Frame::Angle { base, dir } => p = base + dir * (p - base).dot(dir),
            }
        }
        self.point_mods.clear();
        self.feed(Some(Input::Point(p)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters() {
        let a = Axes::parse(".X").unwrap();
        assert_eq!((a.name().as_str(), a.need().as_str()), (".X", "YZ"));
        assert_eq!(a.combine(Vec2::new(1.0, 2.0), Vec2::new(3.0, 4.0)), Vec2::new(1.0, 4.0));
        let a = Axes::parse(".yz").unwrap();
        assert_eq!((a.name().as_str(), a.need().as_str()), (".YZ", "X"));
        assert_eq!(a.combine(Vec2::new(1.0, 2.0), Vec2::new(3.0, 4.0)), Vec2::new(3.0, 2.0));
        assert!(Axes::parse(".5").is_none() && Axes::parse(".w").is_none());
    }
}
