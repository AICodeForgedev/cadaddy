//! Patterns for FIND: plain text, wildcards and regular expressions, matched by one small
//! backtracking matcher with a step budget (a hostile pattern gives up instead of hanging).
//!
//! Regular expressions: literals, `.`, classes `[a-z]` `[^…]`, `\d \w \s \D \W \S \b`, escapes,
//! anchors `^ $`, groups `( )` with `|`, quantifiers `* + ? {m} {m,} {m,n}` (greedy, `?` after one
//! makes it lazy). A replacement may use `$0`–`$9` for the groups.
//!
//! Wildcards (as AutoCAD's): `#` a digit, `@` a letter, `.` a character that is neither, `*` any
//! run of characters, `?` any one, `[…]` one of, `[~…]` none of, `-` a range inside brackets,
//! `` ` `` makes the next character literal.

/// The most steps one search may take.
const MAX_STEPS: usize = 2_000_000;
/// Most repetitions spelled out for `{m,n}`.
const MAX_REPEAT: usize = 100;
/// Longest pattern compiled.
const MAX_PATTERN: usize = 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Plain,
    Wildcard,
    Regex,
}

#[derive(Clone, Debug, PartialEq)]
enum Inst {
    Char(char),
    Any,
    Class(Vec<(char, char)>, bool),
    /// Try the first, then the second.
    Split(usize, usize),
    Jmp(usize),
    Save(usize),
    Start,
    End,
    WordBoundary,
    Match,
}

/// A compiled pattern.
#[derive(Clone, Debug)]
pub struct Pattern {
    prog: Vec<Inst>,
    case: bool,
    whole: bool,
}

/// A match: its byte range and its groups' ranges.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub start: usize,
    pub end: usize,
    pub groups: Vec<Option<(usize, usize)>>,
}

#[derive(Debug)]
enum Node {
    Char(char),
    Any,
    Class(Vec<(char, char)>, bool),
    Start,
    End,
    WordBoundary,
    Group(Box<Node>, usize),
    Alt(Vec<Node>),
    Cat(Vec<Node>),
    Repeat(Box<Node>, usize, Option<usize>, bool),
}

struct Parser<'a> {
    chars: Vec<char>,
    i: usize,
    groups: usize,
    _src: &'a str,
}

fn class_escape(c: char) -> Option<(Vec<(char, char)>, bool)> {
    let digit = vec![('0', '9')];
    let word = vec![('a', 'z'), ('A', 'Z'), ('0', '9'), ('_', '_')];
    let space = vec![(' ', ' '), ('\t', '\t'), ('\n', '\n'), ('\r', '\r'), ('\u{b}', '\u{c}')];
    Some(match c {
        'd' => (digit, false),
        'D' => (digit, true),
        'w' => (word, false),
        'W' => (word, true),
        's' => (space, false),
        'S' => (space, true),
        _ => return None,
    })
}

fn literal_escape(c: char) -> char {
    match c {
        'n' => '\n',
        't' => '\t',
        'r' => '\r',
        other => other,
    }
}

impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.i).copied()
    }
    fn alt(&mut self) -> Result<Node, String> {
        let mut alts = vec![self.cat()?];
        while self.peek() == Some('|') {
            self.i += 1;
            alts.push(self.cat()?);
        }
        Ok(if alts.len() == 1 { alts.remove(0) } else { Node::Alt(alts) })
    }
    fn cat(&mut self) -> Result<Node, String> {
        let mut items = Vec::new();
        while let Some(c) = self.peek() {
            if c == '|' || c == ')' {
                break;
            }
            let atom = self.atom()?;
            items.push(self.quantified(atom)?);
        }
        Ok(Node::Cat(items))
    }
    fn number(&mut self) -> Option<usize> {
        let start = self.i;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.i += 1;
        }
        self.chars.get(start..self.i)?.iter().collect::<String>().parse().ok()
    }
    fn quantified(&mut self, atom: Node) -> Result<Node, String> {
        let (min, max) = match self.peek() {
            Some('*') => (0, None),
            Some('+') => (1, None),
            Some('?') => (0, Some(1)),
            Some('{') => {
                let save = self.i;
                self.i += 1;
                let Some(m) = self.number() else {
                    self.i = save;
                    return Ok(atom);
                };
                let max = if self.peek() == Some(',') {
                    self.i += 1;
                    self.number()
                } else {
                    Some(m)
                };
                if self.peek() != Some('}') {
                    return Err("unclosed {".into());
                }
                if m > MAX_REPEAT || max.is_some_and(|x| x > MAX_REPEAT || x < m) {
                    return Err(format!("repetition over {MAX_REPEAT} or reversed"));
                }
                (m, max)
            }
            _ => return Ok(atom),
        };
        self.i += 1;
        let greedy = if self.peek() == Some('?') {
            self.i += 1;
            false
        } else {
            true
        };
        Ok(Node::Repeat(Box::new(atom), min, max, greedy))
    }
    fn atom(&mut self) -> Result<Node, String> {
        let c = self.peek().ok_or("unexpected end")?;
        self.i += 1;
        Ok(match c {
            '.' => Node::Any,
            '^' => Node::Start,
            '$' => Node::End,
            '(' => {
                // `(?:` groups without capturing.
                let capture = if self.chars.get(self.i..self.i + 2) == Some(&['?', ':']) {
                    self.i += 2;
                    false
                } else {
                    true
                };
                let idx = if capture {
                    self.groups += 1;
                    self.groups
                } else {
                    0
                };
                let inner = self.alt()?;
                if self.peek() != Some(')') {
                    return Err("unclosed (".into());
                }
                self.i += 1;
                if capture { Node::Group(Box::new(inner), idx) } else { inner }
            }
            '[' => self.class()?,
            '\\' => {
                let e = self.peek().ok_or("trailing \\")?;
                self.i += 1;
                if e == 'b' {
                    Node::WordBoundary
                } else if let Some((r, neg)) = class_escape(e) {
                    Node::Class(r, neg)
                } else {
                    Node::Char(literal_escape(e))
                }
            }
            '*' | '+' | '?' => return Err(format!("nothing to repeat before {c}")),
            other => Node::Char(other),
        })
    }
    fn class(&mut self) -> Result<Node, String> {
        let neg = self.peek() == Some('^');
        if neg {
            self.i += 1;
        }
        let mut ranges = Vec::new();
        let mut first = true;
        loop {
            let c = self.peek().ok_or("unclosed [")?;
            self.i += 1;
            if c == ']' && !first {
                break;
            }
            first = false;
            let lo = if c == '\\' {
                let e = self.peek().ok_or("unclosed [")?;
                self.i += 1;
                if let Some((r, false)) = class_escape(e) {
                    ranges.extend(r);
                    continue;
                }
                literal_escape(e)
            } else {
                c
            };
            if self.peek() == Some('-') && self.chars.get(self.i + 1).is_some_and(|c| *c != ']') {
                self.i += 1;
                let hi = self.peek().ok_or("unclosed [")?;
                self.i += 1;
                if hi < lo {
                    return Err("reversed range".into());
                }
                ranges.push((lo, hi));
            } else {
                ranges.push((lo, lo));
            }
        }
        Ok(Node::Class(ranges, neg))
    }
}

fn emit(n: &Node, p: &mut Vec<Inst>) {
    match n {
        Node::Char(c) => p.push(Inst::Char(*c)),
        Node::Any => p.push(Inst::Any),
        Node::Class(r, neg) => p.push(Inst::Class(r.clone(), *neg)),
        Node::Start => p.push(Inst::Start),
        Node::End => p.push(Inst::End),
        Node::WordBoundary => p.push(Inst::WordBoundary),
        Node::Group(inner, idx) => {
            p.push(Inst::Save(2 * idx));
            emit(inner, p);
            p.push(Inst::Save(2 * idx + 1));
        }
        Node::Cat(items) => items.iter().for_each(|i| emit(i, p)),
        Node::Alt(alts) => {
            let mut jumps = Vec::new();
            for (k, a) in alts.iter().enumerate() {
                if k + 1 < alts.len() {
                    let split = p.len();
                    p.push(Inst::Split(split + 1, 0));
                    emit(a, p);
                    jumps.push(p.len());
                    p.push(Inst::Jmp(0));
                    let next = p.len();
                    if let Some(Inst::Split(_, b)) = p.get_mut(split) {
                        *b = next;
                    }
                } else {
                    emit(a, p);
                }
            }
            let end = p.len();
            for j in jumps {
                if let Some(Inst::Jmp(t)) = p.get_mut(j) {
                    *t = end;
                }
            }
        }
        Node::Repeat(inner, min, max, greedy) => {
            for _ in 0..*min {
                emit(inner, p);
            }
            let split = |p: &mut Vec<Inst>, here: usize, body: usize, out: usize| {
                if let Some(s) = p.get_mut(here) {
                    *s = if *greedy { Inst::Split(body, out) } else { Inst::Split(out, body) };
                }
            };
            match max {
                None => {
                    let here = p.len();
                    p.push(Inst::Match);
                    emit(inner, p);
                    p.push(Inst::Jmp(here));
                    let out = p.len();
                    split(p, here, here + 1, out);
                }
                Some(max) => {
                    let mut holes = Vec::new();
                    for _ in *min..*max {
                        let here = p.len();
                        p.push(Inst::Match);
                        emit(inner, p);
                        holes.push(here);
                    }
                    let out = p.len();
                    for h in holes {
                        split(p, h, h + 1, out);
                    }
                }
            }
        }
    }
}

/// Wildcards as a regular expression.
fn wildcard_regex(w: &str) -> String {
    let mut out = String::new();
    let mut chars = w.chars().peekable();
    let esc = |c: char| if "\\.^$|()[]{}*+?".contains(c) { format!("\\{c}") } else { c.to_string() };
    while let Some(c) = chars.next() {
        match c {
            '#' => out.push_str("[0-9]"),
            '@' => out.push_str("[A-Za-z]"),
            '.' => out.push_str("[^A-Za-z0-9]"),
            '*' => out.push_str(".*"),
            '?' => out.push('.'),
            '`' => {
                if let Some(n) = chars.next() {
                    out.push_str(&esc(n));
                }
            }
            '[' => {
                out.push('[');
                if chars.peek() == Some(&'~') {
                    chars.next();
                    out.push('^');
                }
                for n in chars.by_ref() {
                    if n == ']' {
                        break;
                    }
                    if n == '\\' || n == '^' {
                        out.push('\\');
                    }
                    out.push(n);
                }
                out.push(']');
            }
            other => out.push_str(&esc(other)),
        }
    }
    out
}

impl Pattern {
    /// Compile `text` in `mode`; `case`: match case; `whole`: whole words only.
    pub fn new(text: &str, mode: Mode, case: bool, whole: bool) -> Result<Pattern, String> {
        if text.is_empty() {
            return Err("nothing to find".into());
        }
        if text.chars().count() > MAX_PATTERN {
            return Err(format!("a pattern has at most {MAX_PATTERN} characters"));
        }
        let src = match mode {
            Mode::Plain => text.chars().map(|c| if "\\.^$|()[]{}*+?".contains(c) { format!("\\{c}") } else { c.to_string() }).collect(),
            Mode::Wildcard => wildcard_regex(text),
            Mode::Regex => text.to_string(),
        };
        let mut parser = Parser { chars: src.chars().collect(), i: 0, groups: 0, _src: &src };
        let node = parser.alt()?;
        if parser.i < parser.chars.len() {
            return Err("unmatched )".into());
        }
        let mut prog = vec![Inst::Save(0)];
        emit(&node, &mut prog);
        prog.push(Inst::Save(1));
        prog.push(Inst::Match);
        Ok(Pattern { prog, case, whole })
    }

    fn eq(&self, a: char, b: char) -> bool {
        a == b || (!self.case && a.to_lowercase().eq(b.to_lowercase()))
    }

    fn in_class(&self, ranges: &[(char, char)], neg: bool, c: char) -> bool {
        let hit = |c: char| ranges.iter().any(|(lo, hi)| *lo <= c && c <= *hi);
        let found = hit(c) || (!self.case && (c.to_lowercase().any(hit) || c.to_uppercase().any(hit)));
        found != neg
    }

    /// The match starting at char index `at`, by backtracking.
    fn match_at(&self, text: &[char], at: usize, steps: &mut usize) -> Option<Vec<Option<usize>>> {
        let slots = self.prog.iter().filter_map(|i| if let Inst::Save(n) = i { Some(*n + 1) } else { None }).max().unwrap_or(2);
        let mut stack: Vec<(usize, usize, Vec<Option<usize>>)> = vec![(0, at, vec![None; slots])];
        let word = |i: usize| text.get(i).is_some_and(|c| c.is_alphanumeric() || *c == '_');
        while let Some((mut pc, mut pos, mut caps)) = stack.pop() {
            loop {
                *steps += 1;
                if *steps > MAX_STEPS {
                    return None;
                }
                match self.prog.get(pc)? {
                    Inst::Char(c) => match text.get(pos) {
                        Some(t) if self.eq(*t, *c) => {
                            pc += 1;
                            pos += 1;
                        }
                        _ => break,
                    },
                    Inst::Any => match text.get(pos) {
                        Some(t) if *t != '\n' => {
                            pc += 1;
                            pos += 1;
                        }
                        _ => break,
                    },
                    Inst::Class(r, neg) => match text.get(pos) {
                        Some(t) if self.in_class(r, *neg, *t) => {
                            pc += 1;
                            pos += 1;
                        }
                        _ => break,
                    },
                    Inst::Split(a, b) => {
                        stack.push((*b, pos, caps.clone()));
                        pc = *a;
                    }
                    Inst::Jmp(t) => pc = *t,
                    Inst::Save(n) => {
                        if let Some(s) = caps.get_mut(*n) {
                            *s = Some(pos);
                        }
                        pc += 1;
                    }
                    Inst::Start if pos == 0 => pc += 1,
                    Inst::End if pos == text.len() => pc += 1,
                    Inst::Start | Inst::End => break,
                    Inst::WordBoundary => {
                        if (pos > 0 && word(pos - 1)) != word(pos) {
                            pc += 1;
                        } else {
                            break;
                        }
                    }
                    Inst::Match => {
                        let (s, e) = (caps.first().copied().flatten()?, caps.get(1).copied().flatten()?);
                        let whole_ok = !self.whole || ((s == 0 || !word(s - 1)) && !word(e));
                        if whole_ok && (e > s || text.is_empty()) {
                            return Some(caps);
                        }
                        break;
                    }
                }
            }
        }
        None
    }

    /// Every match in `text`, left to right, not overlapping (byte ranges).
    pub fn find_all(&self, text: &str) -> Vec<Hit> {
        let chars: Vec<char> = text.chars().collect();
        let bytes: Vec<usize> = text.char_indices().map(|(b, _)| b).chain(std::iter::once(text.len())).collect();
        let byte = |i: usize| bytes.get(i).copied().unwrap_or(text.len());
        let mut out = Vec::new();
        let mut steps = 0usize;
        let mut at = 0;
        while at <= chars.len() {
            match self.match_at(&chars, at, &mut steps) {
                Some(caps) => {
                    let (s, e) = (caps.first().copied().flatten().unwrap_or(at), caps.get(1).copied().flatten().unwrap_or(at));
                    let groups = (0..caps.len() / 2)
                        .map(|g| Some((byte(caps.get(2 * g).copied().flatten()?), byte(caps.get(2 * g + 1).copied().flatten()?))))
                        .collect();
                    out.push(Hit { start: byte(s), end: byte(e), groups });
                    at = if e > at { e } else { at + 1 };
                }
                None => at += 1,
            }
            if steps > MAX_STEPS || out.len() > 100_000 {
                break;
            }
        }
        out
    }

    /// `text` with every match replaced by `rep` (`$0`–`$9` are the match and its groups when
    /// `groups`), and the number replaced.
    pub fn replace_all(&self, text: &str, rep: &str, groups: bool) -> (String, usize) {
        let hits = self.find_all(text);
        let mut out = String::with_capacity(text.len());
        let mut last = 0;
        for h in &hits {
            out.push_str(text.get(last..h.start).unwrap_or(""));
            if groups {
                let mut chars = rep.chars().peekable();
                while let Some(c) = chars.next() {
                    if c == '$'
                        && let Some(d) = chars.peek().and_then(|d| d.to_digit(10))
                    {
                        chars.next();
                        if let Some(Some((s, e))) = h.groups.get(d as usize) {
                            out.push_str(text.get(*s..*e).unwrap_or(""));
                        }
                    } else {
                        out.push(c);
                    }
                }
            } else {
                out.push_str(rep);
            }
            last = h.end;
        }
        out.push_str(text.get(last..).unwrap_or(""));
        (out, hits.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count(p: &str, mode: Mode, text: &str) -> usize {
        Pattern::new(p, mode, false, false).map(|p| p.find_all(text).len()).unwrap_or(usize::MAX)
    }

    #[test]
    fn plain_wildcard_and_regex() {
        assert_eq!(count("a.b", Mode::Plain, "a.b axb"), 1);
        assert_eq!(count("DOOR", Mode::Plain, "door Door DOOR"), 3);
        assert_eq!(count("D#", Mode::Wildcard, "D1 D2 DX"), 2);
        assert_eq!(count("R@@", Mode::Wildcard, "RAB R12"), 1);
        assert_eq!(count("A*Z", Mode::Wildcard, "A to Z"), 1);
        assert_eq!(count("[~0-9]1", Mode::Wildcard, "A1 21"), 1);
        assert_eq!(count(r"\d{3}-\d{4}", Mode::Regex, "call 555-1234 or 55-1"), 1);
        assert_eq!(count("(cat|dog)s?", Mode::Regex, "cats dog bird"), 2);
        assert_eq!(count("^Room", Mode::Regex, "Room 1"), 1);
        assert_eq!(count("^Room", Mode::Regex, "My Room"), 0);
        let p = Pattern::new(r"(\w+)@(\w+)", Mode::Regex, true, false).unwrap();
        assert_eq!(p.replace_all("joe@site", "$2:$1", true).0, "site:joe");
        let whole = Pattern::new("cat", Mode::Plain, false, true).unwrap();
        assert_eq!(whole.find_all("cat concat cat.").len(), 2);
        for bad in ["(", "[a", "*x", "a{200}", "a{3,1}"] {
            assert!(Pattern::new(bad, Mode::Regex, false, false).is_err(), "{bad}");
        }
        // A pathological pattern gives up instead of hanging.
        let slow = Pattern::new("(a*)*b", Mode::Regex, false, false).unwrap();
        let _ = slow.find_all(&"a".repeat(40));
    }
}
