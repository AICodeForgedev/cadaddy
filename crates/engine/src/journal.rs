//! The session as a script: what was done at the top level (not what commands do inside), one
//! input per line in the SCRIPT format, so replaying it redoes the work. A command started is
//! its name; each input is a line (a point `*x,y` in world coordinates, typed text or a keyword
//! as typed, Enter an empty line); a command cancelled leaves no lines; a call with parameters
//! (from a dialog, an agent) is `command {json}`. Objects picked one by one can't be written as
//! script input and are left out.

use serde_json::Value;

use crate::Input;

/// Most lines kept (the oldest go first).
pub const MAX_LINES: usize = 100_000;

#[derive(Clone, Debug, Default)]
pub struct Journal {
    lines: Vec<String>,
    /// Nesting of `Session::execute`, `start` and `input`: only the outermost is written.
    depth: u32,
    /// Where the running command's lines start (a cancel removes them).
    command: Option<usize>,
}

impl Journal {
    /// The lines, oldest first.
    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    /// The journal as a script file (`.scr`).
    pub fn script(&self) -> String {
        let mut s = self.lines.join("\n");
        s.push('\n');
        s
    }

    pub fn clear(&mut self) {
        self.lines.clear();
        self.command = None;
    }

    /// A call begins; whether it is the outermost.
    pub(crate) fn enter(&mut self) -> bool {
        self.depth = self.depth.saturating_add(1);
        self.depth == 1
    }

    pub(crate) fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    fn push(&mut self, line: String) {
        if self.lines.len() >= MAX_LINES {
            self.lines.remove(0);
            self.command = self.command.map(|c| c.saturating_sub(1));
        }
        self.lines.push(line);
    }

    /// A command started by name.
    pub(crate) fn started(&mut self, name: &str) {
        self.command = Some(self.lines.len());
        self.push(name.trim().to_string());
    }

    /// The command ended: done (its lines stay) or cancelled (they go).
    pub(crate) fn ended(&mut self, done: bool) {
        if let (false, Some(start)) = (done, self.command) {
            self.lines.truncate(start);
        }
        self.command = None;
    }

    /// An input to the running command.
    pub(crate) fn input(&mut self, i: &Input) {
        let line = match i {
            Input::Point(p) => format!("*{},{}", p.x, p.y),
            Input::Point3(p) => format!("{},{},{}", p.x, p.y, p.z),
            Input::Deferred(d) => format!("*{},{}", d.at.x, d.at.y),
            Input::Keyword(t) | Input::Text(t) => t.clone(),
            Input::Enter => String::new(),
            // Picked objects have no script form; a cancel removes the command (`ended`).
            Input::Pick(_) | Input::Cancel => return,
        };
        self.push(line);
    }

    /// A call with parameters made from outside a command.
    pub(crate) fn call(&mut self, id: &str, params: &Value) {
        // Reading the journal or the undo list isn't work to replay.
        let query = id == "commandhistory" || (id == "undohistory" && params.get("action").and_then(Value::as_str) != Some("to"));
        if query {
            return;
        }
        let p = if params.is_null() { "{}".to_string() } else { params.to_string() };
        self.push(format!("{id} {p}"));
    }
}
