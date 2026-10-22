//! A small XML reader for the parts of an XPS (DWFx) package: elements and their attributes as
//! a tree, in any layout (attributes on any line, minified, single or double quotes), with the
//! five predefined entities and character references decoded. Comments, processing
//! instructions, CDATA and the document type are skipped; text content is not kept (the parts
//! CADaddy reads carry everything in attributes). Element names lose their namespace prefix.
//!
//! Hostile input is capped: [`MAX_DEPTH`] levels of nesting and [`MAX_ELEMENTS`] elements.

/// Deepest element nesting read.
pub(crate) const MAX_DEPTH: usize = 256;
/// Most elements read from one part.
pub(crate) const MAX_ELEMENTS: usize = 2_000_000;

/// An element: its local name, its attributes in order (names as written) and its children.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Element {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Element>,
}

impl Element {
    /// The value of the attribute `name` (exact name).
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }
    /// The children named `name`.
    pub fn named<'a, 'n>(&'a self, name: &'n str) -> impl Iterator<Item = &'a Element> + use<'a, 'n> {
        self.children.iter().filter(move |c| c.name == name)
    }
    /// The first child named `name`.
    pub fn child(&self, name: &str) -> Option<&Element> {
        self.named(name).next()
    }
    /// Every element of the tree named `name`, in document order (this one included).
    pub fn descendants<'a>(&'a self, name: &str, out: &mut Vec<&'a Element>) {
        if self.name == name {
            out.push(self);
        }
        for c in &self.children {
            c.descendants(name, out);
        }
    }
}

fn local(name: &str) -> String {
    name.rsplit(':').next().unwrap_or(name).to_string()
}

/// `&amp;` `&lt;` `&gt;` `&quot;` `&apos;` and `&#NN;` / `&#xHH;` decoded; anything else kept.
pub(crate) fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find('&') {
        out.push_str(rest.get(..at).unwrap_or(""));
        let tail = rest.get(at..).unwrap_or("");
        let Some(end) = tail.find(';').filter(|e| *e <= 12) else {
            out.push('&');
            rest = tail.get(1..).unwrap_or("");
            continue;
        };
        let ent = tail.get(1..end).unwrap_or("");
        let ch = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => ent
                .strip_prefix("#x")
                .or_else(|| ent.strip_prefix("#X"))
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| ent.strip_prefix('#').and_then(|d| d.parse::<u32>().ok()))
                .and_then(char::from_u32),
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = tail.get(end + 1..).unwrap_or("");
            }
            None => {
                out.push('&');
                rest = tail.get(1..).unwrap_or("");
            }
        }
    }
    out.push_str(rest);
    out
}

/// Text for an attribute value in double quotes.
pub(crate) fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c if (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r') => {}
            c => out.push(c),
        }
    }
    out
}

/// The root element of `text`.
pub(crate) fn parse(text: &str) -> Result<Element, String> {
    let b = text.as_bytes();
    let mut i = 0usize;
    // Open elements; the bottom one is a holder for the root.
    let mut stack: Vec<Element> = vec![Element::default()];
    let mut count = 0usize;
    let skip_to = |i: usize, pat: &str| -> Result<usize, String> {
        text.get(i..).and_then(|t| t.find(pat)).map(|at| i + at + pat.len()).ok_or_else(|| format!("unterminated `{}`", pat))
    };
    while i < b.len() {
        let Some(at) = text.get(i..).and_then(|t| t.find('<')) else { break };
        i += at;
        let rest = text.get(i..).unwrap_or("");
        if rest.starts_with("<!--") {
            i = skip_to(i + 4, "-->")?;
        } else if rest.starts_with("<![CDATA[") {
            i = skip_to(i + 9, "]]>")?;
        } else if rest.starts_with("<?") {
            i = skip_to(i + 2, "?>")?;
        } else if rest.starts_with("<!") {
            i = skip_to(i + 2, ">")?;
        } else if rest.starts_with("</") {
            let end = skip_to(i + 2, ">")?;
            let name = local(text.get(i + 2..end - 1).unwrap_or("").trim());
            let done = if stack.len() > 1 { stack.pop() } else { None };
            let done = done.ok_or_else(|| format!("`</{name}>` closes nothing"))?;
            if done.name != name {
                return Err(format!("`</{name}>` closes `<{}>`", done.name));
            }
            if let Some(parent) = stack.last_mut() {
                parent.children.push(done);
            }
            i = end;
        } else {
            // A start tag: its name, then attributes up to `>` or `/>`.
            let mut j = i + 1;
            while j < b.len() && !b.get(j).is_some_and(|c| c.is_ascii_whitespace() || *c == b'>' || *c == b'/') {
                j += 1;
            }
            let name = local(text.get(i + 1..j).unwrap_or(""));
            if name.is_empty() {
                return Err("an element without a name".into());
            }
            let mut el = Element { name, attrs: Vec::new(), children: Vec::new() };
            let empty;
            loop {
                while b.get(j).is_some_and(u8::is_ascii_whitespace) {
                    j += 1;
                }
                match b.get(j) {
                    None => return Err(format!("unterminated `<{}`", el.name)),
                    Some(b'>') => {
                        empty = false;
                        j += 1;
                        break;
                    }
                    Some(b'/') if b.get(j + 1) == Some(&b'>') => {
                        empty = true;
                        j += 2;
                        break;
                    }
                    _ => {}
                }
                let start = j;
                while b.get(j).is_some_and(|c| !c.is_ascii_whitespace() && *c != b'=' && *c != b'>' && *c != b'/') {
                    j += 1;
                }
                let an = text.get(start..j).unwrap_or("").to_string();
                while b.get(j).is_some_and(u8::is_ascii_whitespace) {
                    j += 1;
                }
                if an.is_empty() || b.get(j) != Some(&b'=') {
                    return Err(format!("a bad attribute in `<{}`", el.name));
                }
                j += 1;
                while b.get(j).is_some_and(u8::is_ascii_whitespace) {
                    j += 1;
                }
                let q = match b.get(j) {
                    Some(q @ (b'"' | b'\'')) => *q,
                    _ => return Err(format!("attribute `{an}` has no quoted value")),
                };
                let vstart = j + 1;
                let vlen = text.get(vstart..).and_then(|t| t.find(char::from(q))).ok_or_else(|| format!("attribute `{an}` is not closed"))?;
                el.attrs.push((an, unescape(text.get(vstart..vstart + vlen).unwrap_or(""))));
                j = vstart + vlen + 1;
            }
            count += 1;
            if count > MAX_ELEMENTS {
                return Err(format!("more than {MAX_ELEMENTS} elements"));
            }
            if empty {
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(el);
                }
            } else {
                if stack.len() > MAX_DEPTH {
                    return Err(format!("elements nested deeper than {MAX_DEPTH}"));
                }
                stack.push(el);
            }
            i = j;
        }
    }
    if stack.len() > 1 {
        return Err(format!("`<{}>` is not closed", stack.last().map(|e| e.name.as_str()).unwrap_or("")));
    }
    stack.pop().and_then(|h| h.children.into_iter().next()).ok_or_else(|| "no root element".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minified_multiline_and_quoted_any_way() {
        let x = "<?xml version='1.0'?><!-- c --><a:Root xmlns:a='u' A=\"1\"\n  B = '2 &amp; &#x41;&#66;'><Kid\nName=\"k\"/><Kid Name='j'></Kid><![CDATA[<x>]]></a:Root>";
        let r = parse(x).unwrap();
        assert_eq!(r.name, "Root");
        assert_eq!(r.attr("A"), Some("1"));
        assert_eq!(r.attr("B"), Some("2 & AB"));
        let kids: Vec<&str> = r.named("Kid").filter_map(|k| k.attr("Name")).collect();
        assert_eq!(kids, ["k", "j"]);
    }

    #[test]
    fn bad_and_hostile_xml_is_refused() {
        assert!(parse("<a><b></a>").is_err());
        assert!(parse("<a x=1/>").is_err());
        assert!(parse("<a").is_err());
        assert!(parse("<a><!-- </a>").is_err());
        assert!(parse("").is_err());
        let deep = "<a>".repeat(MAX_DEPTH + 2);
        assert!(parse(&deep).unwrap_err().contains("deeper"));
        assert_eq!(escape("a<\"&'>"), "a&lt;&quot;&amp;&apos;&gt;");
        assert_eq!(unescape(&escape("x & <y> \"z\"")), "x & <y> \"z\"");
    }
}
