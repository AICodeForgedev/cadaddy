//! Fields (FIELD): codes written inline in text — `%<\AcVar Date \f "M/d/yyyy">%` — that show the
//! value they last evaluated to. [`Drawing::field_values`] holds those values per object, in the
//! order the codes appear in its text; a code without a value shows as `####`.
//!
//! An object a field refers to is written `%<\_ObjId N>%` inside the code, `N` being the object's
//! handle as a decimal number (DXF files refer to it through the FIELD object instead).

use std::ops::Range;

use crate::Handle;

/// What a field without a value shows.
pub const NO_VALUE: &str = "####";
/// Longest code read as one field (a hostile text can't make the scanner quadratic).
const MAX_CODE: usize = 4096;
/// Most fields in one text.
pub const MAX_FIELDS: usize = 1000;

/// The byte ranges of the top-level field codes in `s` (`%<\…>%`, nested codes included in their
/// parent's range).
pub fn spans(s: &str) -> Vec<Range<usize>> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 2 < b.len() && out.len() < MAX_FIELDS {
        if b.get(i..i + 3) == Some(b"%<\\")
            && let Some(end) = code_end(b, i)
        {
            out.push(i..end);
            i = end;
            continue;
        }
        i += 1;
    }
    out
}

/// Where the code starting at `from` (`%<`) ends (just past its `>%`).
fn code_end(b: &[u8], from: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut j = from;
    while j + 1 < b.len() && j < from + MAX_CODE {
        match (b.get(j), b.get(j + 1)) {
            (Some(b'%'), Some(b'<')) => {
                depth += 1;
                j += 2;
            }
            (Some(b'>'), Some(b'%')) => {
                depth = depth.saturating_sub(1);
                j += 2;
                if depth == 0 {
                    return Some(j);
                }
            }
            _ => j += 1,
        }
    }
    None
}

/// Whether `s` holds a field code.
pub fn has_fields(s: &str) -> bool {
    s.contains("%<\\") && !spans(s).is_empty()
}

/// The codes of `s` without their `%<` `>%`, in order.
pub fn codes(s: &str) -> Vec<&str> {
    spans(s).into_iter().filter_map(|r| s.get(r.start + 2..r.end - 2)).collect()
}

/// `s` with each field code replaced by the next of `values` (`####` once they run out), each
/// value between `marks` when given (so a layout can shade fields).
pub fn substitute<'a>(s: &str, values: &mut impl Iterator<Item = &'a str>, marks: Option<(char, char)>) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last = 0;
    for r in spans(s) {
        out.push_str(s.get(last..r.start).unwrap_or(""));
        let v = values.next().unwrap_or(NO_VALUE);
        match marks {
            Some((on, off)) => {
                out.push(on);
                out.push_str(v);
                out.push(off);
            }
            None => out.push_str(v),
        }
        last = r.end;
    }
    out.push_str(s.get(last..).unwrap_or(""));
    out
}

/// A field code taken apart: `\AcVar Date \f "M/d/yyyy"` is evaluator `AcVar`, expression `Date`
/// and format `M/d/yyyy`.
#[derive(Clone, Debug, PartialEq)]
pub struct Code {
    pub evaluator: String,
    pub expr: String,
    pub format: Option<String>,
}

impl Code {
    /// Parse a code (without its `%<` `>%`).
    pub fn parse(code: &str) -> Code {
        let code = code.trim();
        let body = code.strip_prefix('\\').unwrap_or(code);
        let (evaluator, rest) = match body.find(|c: char| c.is_whitespace()) {
            Some(i) => (body.get(..i).unwrap_or(""), body.get(i..).unwrap_or("")),
            None => (body, ""),
        };
        // The format is the last `\f "…"` outside nested codes.
        let (expr, format) = match format_at(rest) {
            Some((at, f)) => (rest.get(..at).unwrap_or(""), Some(f)),
            None => (rest, None),
        };
        // `\AcObjProp.16.2`: the evaluator's own version suffix.
        let evaluator = evaluator.split('.').next().unwrap_or(evaluator);
        Code { evaluator: evaluator.to_string(), expr: expr.trim().to_string(), format }
    }

    /// The objects the code refers to (`%<\_ObjId N>%`), in order.
    pub fn objects(&self) -> Vec<Handle> {
        object_ids(&self.expr)
    }
}

/// The `\f "…"` format at top level in `s`: its byte position and text.
fn format_at(s: &str) -> Option<(usize, String)> {
    let mut depth = 0usize;
    let b = s.as_bytes();
    let mut found = None;
    let mut i = 0;
    while i < b.len() {
        match (b.get(i), b.get(i + 1)) {
            (Some(b'%'), Some(b'<')) => {
                depth += 1;
                i += 2;
                continue;
            }
            (Some(b'>'), Some(b'%')) => {
                depth = depth.saturating_sub(1);
                i += 2;
                continue;
            }
            (Some(b'\\'), Some(b'f')) if depth == 0 => {
                let rest = s.get(i + 2..).unwrap_or("").trim_start();
                if let Some(q) = rest.strip_prefix('"')
                    && let Some(end) = q.find('"')
                {
                    found = Some((i, q.get(..end).unwrap_or("").to_string()));
                }
            }
            _ => {}
        }
        i += 1;
    }
    found
}

/// The handles of `%<\_ObjId N>%` references in `s`.
pub fn object_ids(s: &str) -> Vec<Handle> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(i) = rest.find("\\_ObjId") {
        let after = rest.get(i + 7..).unwrap_or("");
        let digits: String = after.trim_start().chars().take_while(char::is_ascii_digit).collect();
        if let Ok(n) = digits.parse::<u64>() {
            out.push(Handle(n));
        }
        rest = after;
        if out.len() >= MAX_FIELDS {
            break;
        }
    }
    out
}

/// A code with its object references as indexes (`%<\_ObjIdx i>%`) into the returned handles, as
/// a DXF FIELD object holds them.
pub fn to_indexed(code: &str) -> (String, Vec<Handle>) {
    let mut out = String::with_capacity(code.len());
    let mut handles = Vec::new();
    let mut rest = code;
    while let Some(i) = rest.find("\\_ObjId") {
        out.push_str(rest.get(..i).unwrap_or(""));
        let after = rest.get(i + 7..).unwrap_or("");
        let trimmed = after.trim_start();
        let digits: String = trimmed.chars().take_while(char::is_ascii_digit).collect();
        match digits.parse::<u64>() {
            Ok(n) if after.starts_with(|c: char| c.is_whitespace()) => {
                let idx = handles.iter().position(|h| *h == Handle(n)).unwrap_or_else(|| {
                    handles.push(Handle(n));
                    handles.len() - 1
                });
                out.push_str(&format!("\\_ObjIdx {idx}"));
                rest = trimmed.get(digits.len()..).unwrap_or("");
            }
            _ => {
                out.push_str("\\_ObjId");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    (out, handles)
}

/// The reverse of [`to_indexed`]: `%<\_ObjIdx i>%` becomes `%<\_ObjId N>%` with the handle of
/// `handles[i]` (an index past them is left as it is).
pub fn from_indexed(code: &str, handles: &[Handle]) -> String {
    let mut out = String::with_capacity(code.len());
    let mut rest = code;
    while let Some(i) = rest.find("\\_ObjIdx") {
        out.push_str(rest.get(..i).unwrap_or(""));
        let after = rest.get(i + 8..).unwrap_or("");
        let trimmed = after.trim_start();
        let digits: String = trimmed.chars().take_while(char::is_ascii_digit).collect();
        match digits.parse::<usize>().ok().and_then(|k| handles.get(k)) {
            Some(h) => {
                out.push_str(&format!("\\_ObjId {}", h.0));
                rest = trimmed.get(digits.len()..).unwrap_or("");
            }
            None => {
                out.push_str("\\_ObjIdx");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The field-bearing texts of an entity kind, in the order their values are kept: a text's value,
/// a multiline text's contents, an attribute definition's default, a block reference's attribute
/// values.
pub fn texts(k: &crate::EntityKind) -> Vec<&str> {
    use crate::EntityKind;
    match k {
        EntityKind::Text(t) => vec![t.value.as_str()],
        EntityKind::MText(m) => vec![m.contents.as_str()],
        EntityKind::AttDef(a) => vec![a.text.value.as_str()],
        EntityKind::Insert(ins) => ins.attribs.iter().map(|a| a.text.value.as_str()).collect(),
        _ => Vec::new(),
    }
}

/// Whether an entity kind's text holds a field.
pub fn kind_has_fields(k: &crate::EntityKind) -> bool {
    texts(k).iter().any(|t| has_fields(t))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_and_substitution() {
        let s = "A %<\\AcVar Date \\f \"M/d/yyyy\">% B %<\\AcObjProp Object(%<\\_ObjId 42>%).Area>% C";
        assert_eq!(spans(s).len(), 2);
        assert_eq!(codes(s)[1], "\\AcObjProp Object(%<\\_ObjId 42>%).Area");
        assert_eq!(substitute(s, &mut ["1/2/2026"].into_iter(), None), "A 1/2/2026 B #### C");
        assert_eq!(substitute(s, &mut ["x", "y"].into_iter(), Some(('[', ']'))), "A [x] B [y] C");
        assert_eq!(substitute("plain %< not a field", &mut std::iter::empty(), None), "plain %< not a field");
        assert!(!has_fields("%<\\AcVar Date"), "unterminated");
    }

    #[test]
    fn codes_parse() {
        let c = Code::parse("\\AcVar Date \\f \"M/d/yyyy\"");
        assert_eq!((c.evaluator.as_str(), c.expr.as_str(), c.format.as_deref()), ("AcVar", "Date", Some("M/d/yyyy")));
        let c = Code::parse("\\AcObjProp.16.2 Object(%<\\_ObjId 2130245408>%).Area \\f \"%lu2%pr2\"");
        assert_eq!(
            (c.evaluator.as_str(), c.expr.as_str(), c.format.as_deref()),
            ("AcObjProp", "Object(%<\\_ObjId 2130245408>%).Area", Some("%lu2%pr2"))
        );
        assert_eq!(c.objects(), [Handle(2130245408)]);
        assert_eq!(Code::parse("\\AcVar Filename").format, None);
    }

    #[test]
    fn object_ids_index_and_back() {
        let code = "\\AcObjProp Object(%<\\_ObjId 77>%).Length + Object(%<\\_ObjId 5>%) + Object(%<\\_ObjId 77>%)";
        let (ix, hs) = to_indexed(code);
        assert_eq!(hs, [Handle(77), Handle(5)]);
        assert_eq!(ix, "\\AcObjProp Object(%<\\_ObjIdx 0>%).Length + Object(%<\\_ObjIdx 1>%) + Object(%<\\_ObjIdx 0>%)");
        assert_eq!(from_indexed(&ix, &hs), code);
        assert_eq!(from_indexed("%<\\_ObjIdx 9>%", &hs), "%<\\_ObjIdx 9>%");
    }
}
