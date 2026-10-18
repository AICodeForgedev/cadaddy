//! LEADER and QLEADER at the command line. LEADER: points until Enter, then lines of annotation
//! text until an empty one. QLEADER (LE): up to three points (Enter ends sooner), the text width,
//! then the text lines, as AutoCAD's quick leader asks with its default settings.

use serde_json::json;

use super::run_leader;
use crate::{Accept, Input, Interactive, Prompt, Result, Session, Step};
use cadaddy_doc::EntityKind;
use cadaddy_geom::Vec2;

/// QLEADER's default maximum number of leader points.
const QUICK_POINTS: usize = 3;

pub(super) struct LeaderM {
    quick: bool,
    pts: Vec<Vec2>,
    /// The points are placed: asking for the text (QLEADER first asks for its width).
    texting: bool,
    width_asked: bool,
    lines: Vec<String>,
}

impl LeaderM {
    pub(super) fn new(quick: bool) -> Self {
        LeaderM { quick, pts: Vec::new(), texting: false, width_asked: false, lines: Vec::new() }
    }
    fn finish(&mut self, s: &mut Session) -> Result<Step> {
        let pts: Vec<[f64; 2]> = self.pts.iter().map(|p| [p.x, p.y]).collect();
        let mut p = json!({ "points": pts });
        if !self.lines.is_empty() {
            p["text"] = json!(self.lines.join("\\P"));
        }
        run_leader(s, &p)?;
        Ok(Step::Done)
    }
    fn points_done(&mut self) {
        self.texting = true;
        self.width_asked = !self.quick;
    }
}

impl Interactive for LeaderM {
    fn name(&self) -> &'static str {
        if self.quick { "QLEADER" } else { "LEADER" }
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        if self.texting && !self.width_asked {
            return Prompt::new("Specify text width", Accept::POINT_OR_NUMBER).default("0").base_opt(self.pts.last().copied());
        }
        if self.texting {
            return if self.lines.is_empty() {
                Prompt::new("Enter first line of annotation text", Accept::TEXT)
            } else {
                Prompt::new("Enter next line of annotation text", Accept::TEXT)
            };
        }
        match self.pts.last() {
            None if self.quick => Prompt::new("Specify first leader point", Accept::POINT),
            None => Prompt::new("Specify leader start point", Accept::POINT),
            Some(l) => Prompt::new("Specify next point", Accept::POINT).base(*l),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if matches!(i, Input::Cancel) {
            return Ok(Step::Cancel);
        }
        if self.texting && !self.width_asked {
            // The width only shapes AutoCAD's MText box; the text here is not wrapped.
            if matches!(i, Input::Enter | Input::Text(_) | Input::Point(_)) {
                self.width_asked = true;
            }
            return Ok(Step::Continue);
        }
        if self.texting {
            return match i {
                Input::Text(t) if !t.is_empty() => {
                    self.lines.push(t);
                    Ok(Step::Continue)
                }
                Input::Text(_) | Input::Enter => self.finish(s),
                _ => Ok(Step::Continue),
            };
        }
        match i {
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
        }
    }
    fn preview(&self, _s: &Session, c: Vec2) -> Vec<EntityKind> {
        if self.texting {
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
