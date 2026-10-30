//! The session as a script: what was done at the top level (not what commands do inside), one
//! input per line in the SCRIPT format, so replaying it redoes the work. A command started is
//! its name; each input is a line (a point `*x,y` in world coordinates, typed text or a keyword
//! as typed, Enter an empty line); a command cancelled leaves no lines; a call with parameters
//! (from a dialog, an agent) is `command {json}`. Objects picked one by one can't be written as
//! script input and are left out (their positions are kept, so a recording can say so).

use serde_json::Value;

use crate::Input;

/// Most lines kept (the oldest go first).
pub const MAX_LINES: usize = 100_000;
/// Most bytes of text kept: an agent's calls can carry megabytes of parameters each, and a
/// session kept 100,000 of them (Q240).
pub const MAX_BYTES: usize = 16 << 20;

#[derive(Clone, Debug, Default)]
pub struct Journal {
    lines: Vec<String>,
    /// Bytes of text in `lines`.
    bytes: usize,
    /// Lines let go from the front so far: positions ([`Self::position`]) count them, so a mark
    /// made before the oldest lines went still points at the same line.
    dropped: usize,
    /// Nesting of `Session::execute`, `start` and `input`: only the outermost is written.
    depth: u32,
    /// Where the running command's lines start (a cancel removes them).
    command: Option<usize>,
    /// Positions at which objects were picked one by one: they have no script form, so no line
    /// was written, but a recording must know where they were left out.
    picks: Vec<usize>,
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

    /// No call is running: the next one is the outermost.
    pub fn is_outer(&self) -> bool {
        self.depth == 0
    }

    /// The position the next line will have: lines written since (and still kept) are
    /// [`Self::lines_from`] it.
    pub fn position(&self) -> usize {
        self.dropped.saturating_add(self.lines.len())
    }

    /// The position of the oldest line kept.
    pub fn first_position(&self) -> usize {
        self.dropped
    }

    /// The lines kept from `position` on.
    pub fn lines_from(&self, position: usize) -> &[String] {
        self.lines.get(position.saturating_sub(self.dropped)..).unwrap_or_default()
    }

    /// Positions of the picks left out of the lines from `position` on (oldest first).
    pub fn picks_from(&self, position: usize) -> impl Iterator<Item = usize> + '_ {
        self.picks.iter().copied().filter(move |at| *at >= position)
    }

    /// Bytes of text kept.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn clear(&mut self) {
        self.dropped = self.position();
        self.lines.clear();
        self.bytes = 0;
        self.command = None;
        self.picks.clear();
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
        self.bytes = self.bytes.saturating_add(line.len());
        self.lines.push(line);
        if self.lines.len() > MAX_LINES || self.bytes > MAX_BYTES {
            // The oldest tenth go at once (one line at a time moved the whole journal on every
            // line once it was full), and more until the text is within its budget; the newest
            // line always stays.
            let keep_last = self.lines.len().saturating_sub(1);
            let mut n = (self.lines.len() / 10).clamp(1, keep_last.max(1));
            let mut freed: usize = self.lines.iter().take(n).map(String::len).sum();
            while self.bytes.saturating_sub(freed) > MAX_BYTES && n < keep_last {
                freed += self.lines.get(n).map_or(0, String::len);
                n += 1;
            }
            let n = n.min(keep_last);
            if n > 0 {
                let freed: usize = self.lines.drain(..n).map(|l| l.len()).sum();
                self.bytes = self.bytes.saturating_sub(freed);
                self.dropped = self.dropped.saturating_add(n);
                self.command = self.command.map(|c| c.saturating_sub(n));
                let first = self.dropped;
                self.picks.retain(|at| *at >= first);
            }
        }
    }

    /// A command started by name.
    pub(crate) fn started(&mut self, name: &str) {
        self.command = Some(self.lines.len());
        self.push(name.trim().to_string());
    }

    /// The command ended: done (its lines stay) or cancelled (they go).
    pub(crate) fn ended(&mut self, done: bool) {
        if let (false, Some(start)) = (done, self.command) {
            let freed: usize = self.lines.get(start..).map_or(0, |l| l.iter().map(String::len).sum());
            self.lines.truncate(start);
            self.bytes = self.bytes.saturating_sub(freed);
            let from = self.dropped.saturating_add(start);
            self.picks.retain(|at| *at < from);
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
            // Picked objects have no script form: only where they were left out is kept (a
            // cancel removes the command, `ended`).
            Input::Pick(hs) => {
                if !hs.is_empty() && self.picks.len() < MAX_LINES {
                    self.picks.push(self.position());
                }
                return;
            }
            Input::Cancel => return,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_journal_keeps_to_its_budget_and_its_positions() {
        let mut j = Journal::default();
        j.started("line");
        let mark = j.position();
        j.input(&Input::Text("0,0".into()));
        assert_eq!(j.lines_from(mark), ["0,0".to_string()]);
        // Calls with a megabyte of parameters each: the oldest go, the text stays in budget.
        let big = serde_json::json!({ "text": "x".repeat(1 << 20) });
        for _ in 0..40 {
            j.call("mtext", &big);
        }
        assert!(j.bytes() <= MAX_BYTES, "{} bytes kept", j.bytes());
        assert_eq!(j.bytes(), j.lines().iter().map(String::len).sum::<usize>());
        assert!(j.first_position() > mark, "the oldest lines went");
        assert!(j.lines_from(mark).len() == j.lines().len(), "a mark before the oldest line gives all that is kept");
        assert_eq!(j.position(), j.first_position() + j.lines().len());
        // A line bigger than the whole budget is still the newest line.
        j.call("mtext", &serde_json::json!({ "text": "y".repeat(MAX_BYTES + 1) }));
        assert_eq!(j.lines().len(), 1);
        // A cancelled command's lines go, and their bytes with them.
        j.clear();
        let at = j.position();
        j.started("circle");
        j.input(&Input::Text("5".into()));
        j.ended(false);
        assert_eq!((j.lines().len(), j.bytes(), j.position()), (0, 0, at));
    }

    #[test]
    fn a_full_journal_lets_go_of_lines_in_chunks() {
        let mut j = Journal::default();
        for i in 0..MAX_LINES + 5 {
            j.call("line", &serde_json::json!({ "i": i }));
        }
        assert!(j.lines().len() <= MAX_LINES && j.lines().len() > MAX_LINES * 8 / 10);
        assert_eq!(j.position(), MAX_LINES + 5);
        assert_eq!(j.lines().last().map(String::as_str), Some(format!("line {{\"i\":{}}}", MAX_LINES + 4).as_str()));
    }
}
