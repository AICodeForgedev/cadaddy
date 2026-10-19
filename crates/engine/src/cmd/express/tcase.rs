//! TCASE: change the case of text, multiline text and attribute values: Sentence case,
//! lowercase, UPPERCASE, Title, or tOGGLE cASE. Only the characters shown change: MTEXT format
//! codes (`\P`, `\fArial;`, `\U+00B0`…), `%%` codes and field codes stay as they are.

use cadaddy_doc::{EntityKind, Handle};
use serde_json::{Value, json};

use super::super::machines::{SelOutcome, SelectPhase};
use super::super::*;
use crate::{Accept, Input, Interactive, Prompt, Result, Session, Step};

/// Most objects one call changes.
const MAX_OBJECTS: usize = 100_000;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Case {
    Sentence,
    Lower,
    Upper,
    Title,
    Toggle,
}

impl Case {
    fn parse(t: &str) -> Option<Case> {
        Some(match t.trim().to_ascii_lowercase().as_str() {
            "sentence" | "s" => Case::Sentence,
            "lower" | "lowercase" | "l" => Case::Lower,
            "upper" | "uppercase" | "u" => Case::Upper,
            "title" | "t" => Case::Title,
            "toggle" | "o" => Case::Toggle,
            _ => return None,
        })
    }
}

/// For each character of `chars`, whether it is shown text (not a format, `%%` or field code).
fn literal_mask(chars: &[char], mtext: bool) -> Vec<bool> {
    let mut mask = vec![true; chars.len()];
    let mut i = 0;
    while i < chars.len() {
        let code = if let Some(n) = cadaddy_fonts::field_code(chars, i) {
            n
        } else if let Some((_, n)) = cadaddy_fonts::unicode_escape(chars, i) {
            n
        } else if chars.get(i) == Some(&'%') && chars.get(i + 1) == Some(&'%') {
            match chars.get(i + 2) {
                Some(c) if c.is_ascii_digit() => 2 + chars[i + 2..].iter().take(3).take_while(|c| c.is_ascii_digit()).count(),
                Some(_) => 3,
                None => 2,
            }
        } else if mtext && matches!(chars.get(i), Some('{' | '}')) {
            1
        } else if mtext && chars.get(i) == Some(&'\\') {
            match chars.get(i + 1) {
                None => 1,
                // Escaped characters and one-letter codes.
                Some('\\' | '{' | '}' | 'P' | 'N' | 'L' | 'l' | 'O' | 'o' | 'K' | 'k' | 'X' | '~') => 2,
                // Codes with a value, up to `;`.
                Some(_) => chars[i..].iter().position(|c| *c == ';').map_or(chars.len() - i, |p| p + 1),
            }
        } else {
            0
        };
        if code == 0 {
            i += 1;
            continue;
        }
        let end = (i + code).min(chars.len());
        mask[i..end].iter_mut().for_each(|m| *m = false);
        i = end;
    }
    mask
}

fn upper(c: char) -> String {
    c.to_uppercase().collect()
}

fn lower(c: char) -> String {
    c.to_lowercase().collect()
}

/// `s` in case `case`. `mtext` reads MTEXT format codes.
pub(crate) fn change_case(s: &str, case: Case, mtext: bool) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mask = literal_mask(&chars, mtext);
    let mut out = String::with_capacity(s.len());
    // Sentence: the next letter starts a sentence. Title: the next letter starts a word.
    let mut start = true;
    for (c, lit) in chars.iter().zip(&mask) {
        if !lit {
            out.push(*c);
            // A paragraph break starts a new sentence too.
            continue;
        }
        let c = *c;
        match case {
            Case::Upper => out.push_str(&upper(c)),
            Case::Lower => out.push_str(&lower(c)),
            Case::Toggle if c.is_uppercase() => out.push_str(&lower(c)),
            Case::Toggle => out.push_str(&upper(c)),
            Case::Sentence | Case::Title => {
                if c.is_alphabetic() {
                    out.push_str(&if start { upper(c) } else { lower(c) });
                    start = false;
                } else {
                    out.push(c);
                    if case == Case::Title && (c.is_whitespace() || c == '-' || c == '/' || c == '(') {
                        start = true;
                    }
                    if case == Case::Sentence && matches!(c, '.' | '!' | '?') {
                        start = true;
                    }
                }
            }
        }
    }
    out
}

/// Change the case of object `h`; whether it was text-based.
fn apply(s: &mut Session, h: Handle, case: Case) -> Result<bool> {
    let d = s.doc()?;
    let Some(e) = d.entity(h) else { return Ok(false) };
    let mut k = e.kind.clone();
    let changed = match &mut k {
        EntityKind::Text(t) => {
            t.value = change_case(&t.value, case, false);
            true
        }
        EntityKind::AttDef(a) => {
            a.text.value = change_case(&a.text.value, case, false);
            true
        }
        EntityKind::MText(t) => {
            t.contents = change_case(&t.contents, case, true);
            true
        }
        // A block reference: its attribute values.
        EntityKind::Insert(ins) if !ins.attribs.is_empty() => {
            for a in &mut ins.attribs {
                a.text.value = change_case(&a.text.value, case, false);
            }
            true
        }
        _ => false,
    };
    if changed && k != e.kind {
        s.doc_mut()?.modify_entity(h, |e| e.kind = k)?;
    }
    Ok(changed)
}

pub(crate) fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let case = match p.get("case") {
        None | Some(Value::Null) => Case::Sentence,
        Some(v) => v.as_str().and_then(Case::parse).ok_or_else(|| bad("tcase", "`case` is sentence, lower, upper, title or toggle"))?,
    };
    let hs = targets(s, p)?;
    let mut changed = Vec::new();
    for h in hs.into_iter().take(MAX_OBJECTS) {
        if super::super::curves::is_locked(s, h) {
            continue;
        }
        if apply(s, h, case)? {
            changed.push(h.hex());
        }
    }
    if changed.is_empty() {
        return Err(other("No text, mtext or attributes selected."));
    }
    Ok(json!({ "handles": changed }))
}

#[derive(Default)]
pub(crate) struct TcaseM {
    sel: SelectPhase,
    objs: Vec<Handle>,
}

impl Interactive for TcaseM {
    fn name(&self) -> &'static str {
        "TCASE"
    }
    fn begin(&mut self, s: &mut Session) -> Result<Step> {
        self.sel = SelectPhase::begin(s);
        if self.sel.done {
            self.objs = self.sel.picked.clone();
        }
        Ok(Step::Continue)
    }
    fn prompt(&self, _s: &Session) -> Prompt {
        if !self.sel.done {
            return self.sel.prompt();
        }
        Prompt::new("Select case", Accept { point: false, number: false, text: false, select: false, enter: true })
            .kw(&["Sentence", "Lower", "Upper", "Title", "tOggle"])
            .default("Sentence")
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if !self.sel.done {
            return match self.sel.feed(s, &i)? {
                SelOutcome::More => Ok(Step::Continue),
                SelOutcome::Empty => Ok(Step::Cancel),
                SelOutcome::Done(hs) => {
                    self.objs = hs;
                    Ok(Step::Continue)
                }
            };
        }
        let case = match &i {
            Input::Enter => Case::Sentence,
            Input::Keyword(k) | Input::Text(k) => match k.as_str() {
                "tOggle" => Case::Toggle,
                k => Case::parse(k).ok_or_else(|| other("Invalid option keyword."))?,
            },
            Input::Cancel => return Ok(Step::Cancel),
            _ => return Ok(Step::Continue),
        };
        let r = run(
            s,
            &json!({ "handles": self.objs.iter().map(|h| h.hex()).collect::<Vec<_>>(), "case": match case {
            Case::Sentence => "sentence",
            Case::Lower => "lower",
            Case::Upper => "upper",
            Case::Title => "title",
            Case::Toggle => "toggle",
        } }),
        )?;
        let n = r["handles"].as_array().map_or(0, Vec::len);
        s.echo(format!("{n} object(s) changed."));
        s.set_selection(Vec::new());
        Ok(Step::Done)
    }
}

fn other(m: impl Into<String>) -> crate::EngineError {
    crate::EngineError::Other(m.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cases_leave_codes_alone() {
        assert_eq!(change_case("the quick. brown fox! jumps", Case::Sentence, false), "The quick. Brown fox! Jumps");
        assert_eq!(change_case("NEW panel-board a1", Case::Title, false), "New Panel-Board A1");
        assert_eq!(change_case("AbC 1%%d", Case::Toggle, false), "aBc 1%%d");
        assert_eq!(change_case("45%%D x", Case::Upper, false), "45%%D X");
        assert_eq!(change_case("{\\fArial|b1;Hello}\\PWorld \\U+00e9", Case::Upper, true), "{\\fArial|b1;HELLO}\\PWORLD \\U+00e9");
        assert_eq!(change_case("ab %<\\AcVar Date>% cd", Case::Upper, false), "AB %<\\AcVar Date>% CD");
        assert_eq!(change_case("\\", Case::Upper, true), "\\");
        assert_eq!(change_case("ÉCOLE straße", Case::Lower, false), "école straße");
    }
}
