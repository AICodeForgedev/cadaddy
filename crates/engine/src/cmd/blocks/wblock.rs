//! WBLOCK at the command line (AutoCAD's -WBLOCK): the output file, then an existing block, `=`
//! (the block named like the file), `*` (the whole drawing) or Enter for new objects, which asks
//! for their base point and selects them. A file name without an extension gets `.dwg`.

use cadaddy_geom::Vec2;
use serde_json::{Value, json};

use crate::{Accept, Input, Interactive, Prompt, Result, Session, Step};

#[derive(Default)]
pub(super) struct WblockM {
    path: Option<String>,
    base: Option<Vec2>,
    /// Asking for the base point (after Enter at the block prompt).
    picking: bool,
    handles: Vec<String>,
}

/// `path` with `.dwg` added when it has no extension (AutoCAD saves a drawing file).
pub(super) fn with_extension(path: &str) -> String {
    let p = std::path::Path::new(path);
    if p.extension().is_none() && p.file_name().is_some() { format!("{path}.dwg") } else { path.to_string() }
}

fn stem(path: &str) -> String {
    std::path::Path::new(path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

impl WblockM {
    fn write(&self, s: &mut Session, p: Value) -> Result<Step> {
        let r = super::run_wblock(s, &p)?;
        let path = r.get("path").and_then(Value::as_str).or(self.path.as_deref()).unwrap_or_default().to_string();
        s.echo(format!("Wrote {path}"));
        Ok(Step::Done)
    }
}

impl Interactive for WblockM {
    fn name(&self) -> &'static str {
        "WBLOCK"
    }

    fn prompt(&self, _s: &Session) -> Prompt {
        if self.path.is_none() {
            Prompt::new("Enter name of output file", Accept::TEXT)
        } else if self.base.is_some() {
            Prompt::new("Select objects", Accept::SELECT)
        } else if self.picking {
            Prompt::new("Specify insertion base point", Accept::POINT).default("0,0")
        } else {
            Prompt::new("Enter name of existing block or [= (block=output file)/* (whole drawing)] <define new drawing>", Accept::TEXT)
        }
    }

    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if i == Input::Cancel {
            return Ok(Step::Cancel);
        }
        let Some(path) = self.path.clone() else {
            match i {
                Input::Text(t) | Input::Keyword(t) if !t.trim().is_empty() => self.path = Some(with_extension(t.trim().trim_matches('"'))),
                Input::Enter | Input::Text(_) => return Ok(Step::Cancel),
                _ => {}
            }
            return Ok(Step::Continue);
        };
        if self.base.is_none() && !self.picking {
            return match i {
                Input::Enter => {
                    self.picking = true;
                    Ok(Step::Continue)
                }
                Input::Text(t) | Input::Keyword(t) => match t.trim() {
                    "" => {
                        self.picking = true;
                        Ok(Step::Continue)
                    }
                    "*" => {
                        let d = s.doc()?;
                        let all: Vec<String> = d.model.iter().map(|e| e.handle.hex()).collect();
                        self.write(s, json!({ "path": path, "handles": all }))
                    }
                    "=" => self.write(s, json!({ "path": path, "name": stem(&path) })),
                    name => self.write(s, json!({ "path": path, "name": name })),
                },
                _ => Ok(Step::Continue),
            };
        }
        if self.base.is_none() {
            match i {
                Input::Point(p) => self.base = Some(p),
                Input::Enter => self.base = Some(Vec2::ZERO),
                Input::Text(_) | Input::Keyword(_) => s.echo("Point or option keyword required."),
                _ => {}
            }
            return Ok(Step::Continue);
        }
        match i {
            Input::Pick(hs) => {
                let mut have: std::collections::HashSet<String> = self.handles.iter().cloned().collect();
                for h in hs {
                    let h = h.hex();
                    if have.insert(h.clone()) {
                        self.handles.push(h);
                    }
                }
                Ok(Step::Continue)
            }
            Input::Enter => {
                if self.handles.is_empty() {
                    s.echo("Nothing selected.");
                    return Ok(Step::Done);
                }
                let b = self.base.unwrap_or(Vec2::ZERO);
                self.write(s, json!({ "path": path, "handles": self.handles, "base": [b.x, b.y] }))
            }
            Input::Text(_) | Input::Keyword(_) => {
                s.echo("*Invalid selection*");
                Ok(Step::Continue)
            }
            _ => Ok(Step::Continue),
        }
    }
}
