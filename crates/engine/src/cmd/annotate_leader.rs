//! LEADER and QLEADER at the command line. LEADER: points until Enter, then lines of annotation
//! text until an empty one; Enter at the first line offers the annotation options (Tolerance,
//! None, Mtext). QLEADER (LE): Settings chooses the annotation type (Mtext, Tolerance, None); up
//! to three points (Enter ends sooner), then the text width and lines, or the frame questions.

use serde_json::json;

use super::run_leader;
use crate::{Accept, Input, Interactive, Prompt, Result, Session, Step};
use cadaddy_doc::EntityKind;
use cadaddy_geom::Vec2;

/// QLEADER's default maximum number of leader points.
const QUICK_POINTS: usize = 3;

/// What a leader ends with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Annotation {
    Mtext,
    Tolerance,
    None,
}

/// Where the questions are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Points,
    /// QLEADER Settings: the annotation type.
    Type,
    /// LEADER, Enter at the first text line: the annotation options.
    Options,
    Width,
    Text,
    Frame,
}

pub(super) struct LeaderM {
    quick: bool,
    pts: Vec<Vec2>,
    stage: Stage,
    annotation: Annotation,
    lines: Vec<String>,
    frame: super::tolerance::FrameAsk,
}

impl LeaderM {
    pub(super) fn new(quick: bool) -> Self {
        LeaderM { quick, pts: Vec::new(), stage: Stage::Points, annotation: Annotation::Mtext, lines: Vec::new(), frame: Default::default() }
    }
    fn finish(&mut self, s: &mut Session, tolerance: Option<String>) -> Result<Step> {
        let pts: Vec<[f64; 2]> = self.pts.iter().map(|p| [p.x, p.y]).collect();
        let mut p = json!({ "points": pts });
        if let Some(t) = tolerance {
            p["tolerance"] = json!(t);
        } else if !self.lines.is_empty() {
            p["text"] = json!(self.lines.join("\\P"));
        }
        run_leader(s, &p)?;
        Ok(Step::Done)
    }
    fn points_done(&mut self) {
        self.stage = match (self.quick, self.annotation) {
            (_, Annotation::Tolerance) => Stage::Frame,
            (true, Annotation::Mtext) => Stage::Width,
            (false, Annotation::Mtext) => Stage::Text,
            (_, Annotation::None) => Stage::Text,
        };
    }
}

const ANNOTATION_KW: &[&str] = &["Mtext", "Tolerance", "None"];
const KW: Accept = Accept { point: false, number: false, text: false, select: false, enter: true };

impl Interactive for LeaderM {
    fn name(&self) -> &'static str {
        if self.quick { "QLEADER" } else { "LEADER" }
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        match self.stage {
            Stage::Type => Prompt::new("Enter annotation type", KW).kw(ANNOTATION_KW).default("Mtext"),
            Stage::Options => Prompt::new("Enter an annotation option", KW).kw(ANNOTATION_KW).default("Mtext"),
            Stage::Width => Prompt::new("Specify text width", Accept::POINT_OR_NUMBER).default("0").base_opt(self.pts.last().copied()),
            Stage::Frame => self.frame.prompt(),
            Stage::Text if self.lines.is_empty() && !self.quick => Prompt::new("Enter first line of annotation text or <options>", Accept::TEXT),
            Stage::Text if self.lines.is_empty() => Prompt::new("Enter first line of annotation text", Accept::TEXT),
            Stage::Text => Prompt::new("Enter next line of annotation text", Accept::TEXT),
            Stage::Points => match self.pts.last() {
                None if self.quick => Prompt::new("Specify first leader point, or", Accept::POINT).kw(&["Settings"]),
                None => Prompt::new("Specify leader start point", Accept::POINT),
                Some(l) => Prompt::new("Specify next point", Accept::POINT).base(*l),
            },
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if matches!(i, Input::Cancel) {
            return Ok(Step::Cancel);
        }
        let choice = |i: &Input| -> Option<Annotation> {
            match i {
                Input::Enter => Some(Annotation::Mtext),
                Input::Keyword(k) | Input::Text(k) => match k.trim().to_ascii_lowercase().chars().next() {
                    Some('m') => Some(Annotation::Mtext),
                    Some('t') => Some(Annotation::Tolerance),
                    Some('n') => Some(Annotation::None),
                    _ => None,
                },
                _ => None,
            }
        };
        match self.stage {
            Stage::Type => {
                let Some(a) = choice(&i) else { return Err(crate::EngineError::Other("Invalid option keyword.".into())) };
                self.annotation = a;
                self.stage = Stage::Points;
                Ok(Step::Continue)
            }
            Stage::Options => match choice(&i) {
                Some(Annotation::Mtext) => {
                    self.stage = Stage::Text;
                    self.annotation = Annotation::Mtext;
                    // Back at the text prompt, Enter now ends with no text.
                    self.lines.clear();
                    self.quick = true;
                    Ok(Step::Continue)
                }
                Some(Annotation::Tolerance) => {
                    self.annotation = Annotation::Tolerance;
                    self.stage = Stage::Frame;
                    Ok(Step::Continue)
                }
                Some(Annotation::None) => self.finish(s, None),
                None => Err(crate::EngineError::Other("Invalid option keyword.".into())),
            },
            Stage::Width => {
                // The width only shapes AutoCAD's MText box; the text here is not wrapped.
                if matches!(i, Input::Enter | Input::Text(_) | Input::Point(_)) {
                    self.stage = Stage::Text;
                }
                Ok(Step::Continue)
            }
            Stage::Frame => match self.frame.input(s, &i) {
                super::tolerance::Asked::More => Ok(Step::Continue),
                super::tolerance::Asked::Cancel => Ok(Step::Cancel),
                super::tolerance::Asked::Done(t) => self.finish(s, Some(t)),
            },
            Stage::Text if self.annotation == Annotation::None => self.finish(s, None),
            Stage::Text => match i {
                Input::Text(t) if !t.is_empty() => {
                    self.lines.push(t);
                    Ok(Step::Continue)
                }
                // LEADER: Enter at the first line offers the annotation options.
                Input::Text(_) | Input::Enter if self.lines.is_empty() && !self.quick => {
                    self.stage = Stage::Options;
                    Ok(Step::Continue)
                }
                Input::Text(_) | Input::Enter => self.finish(s, None),
                _ => Ok(Step::Continue),
            },
            Stage::Points => match i {
                Input::Keyword(k) if self.quick && self.pts.is_empty() && k.eq_ignore_ascii_case("Settings") => {
                    self.stage = Stage::Type;
                    Ok(Step::Continue)
                }
                Input::Point(p) => {
                    if self.pts.last().is_some_and(|l| l.near(p, 1e-12)) {
                        return Ok(Step::Continue);
                    }
                    self.pts.push(p);
                    if self.quick && self.pts.len() >= QUICK_POINTS {
                        self.points_done();
                    }
                    Ok(Step::Continue)
                }
                Input::Enter if self.pts.len() >= 2 => {
                    self.points_done();
                    Ok(Step::Continue)
                }
                Input::Enter => Ok(Step::Cancel),
                _ => Ok(Step::Continue),
            },
        }
    }
    fn preview(&self, _s: &Session, c: Vec2) -> Vec<EntityKind> {
        if self.stage != Stage::Points {
            return Vec::new();
        }
        self.pts.last().map(|l| vec![super::helpers::line(*l, c)]).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use cadaddy_doc::EntityKind;

    use crate::Session;

    fn counts(s: &Session) -> (usize, Vec<String>) {
        let d = s.doc().unwrap();
        let leaders = d.model.iter().filter(|e| matches!(e.kind, EntityKind::Leader(_))).count();
        let texts = d.model.iter().filter_map(|e| if let EntityKind::MText(m) = &e.kind { Some(m.contents.clone()) } else { None }).collect();
        (leaders, texts)
    }

    #[test]
    fn leader_and_qleader_at_the_command_line() {
        let mut s = Session::new();
        s.script("LEADER 0,0 5,5 8,5\n\nPANEL LP-1\nCKT 12\n\n").unwrap();
        assert!(s.running.is_none(), "{}", s.prompt_text());
        assert_eq!(counts(&s), (1, vec!["PANEL LP-1\\PCKT 12".to_string()]));
        // QLEADER: the third point ends the points; then width and text.
        let mut s = Session::new();
        s.script("QLEADER 0,0 5,5 8,5\n0\nNOTE 1\n\n").unwrap();
        assert!(s.running.is_none(), "{}", s.prompt_text());
        assert_eq!(counts(&s), (1, vec!["NOTE 1".to_string()]));
        // LE is QLEADER; Enter after two points ends them.
        let mut s = Session::new();
        s.script("LE 0,0 5,5\n\n\nX\n\n").unwrap();
        assert!(s.running.is_none(), "{}", s.prompt_text());
        assert_eq!(counts(&s).0, 1);
    }
}
