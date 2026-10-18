//! The SVG reader's document model: the element tree (so `use` can reach any element and
//! `<style>` applies wherever it sits), simple CSS rules, and the property values the reader
//! needs (paint, colours, font sizes, text anchors).

use std::collections::HashMap;

use cadaddy_color::Rgb;
use quick_xml::Reader;
use quick_xml::XmlVersion;
use quick_xml::events::{BytesStart, Event};

use crate::{IoError, Result};

/// Elements per file (the tree is built before anything is read from it).
const MAX_NODES: usize = 2_000_000;
/// Element nesting.
const MAX_DEPTH: usize = 1024;
/// CSS rules kept from all `<style>` elements together.
const MAX_RULES: usize = 20_000;
/// Elements whose text content is kept (whitespace included).
const TEXT_HOLDERS: &[&str] = &["text", "tspan", "style", "textPath"];

pub(crate) struct Node {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub kids: Vec<Kid>,
}

pub(crate) enum Kid {
    El(usize),
    Text(String),
}

impl Node {
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    /// `href`, `xlink:href`, or the `href` of whatever prefix the file bound XLink to.
    pub fn href(&self) -> Option<&str> {
        self.attr("href")
            .or_else(|| self.attr("xlink:href"))
            .or_else(|| self.attrs.iter().find(|(k, _)| k.ends_with(":href")).map(|(_, v)| v.as_str()))
    }

    /// The element's own text, without its child elements'.
    pub fn own_text(&self) -> String {
        self.kids.iter().filter_map(|k| if let Kid::Text(t) = k { Some(t.as_str()) } else { None }).collect()
    }
}

pub(crate) struct Dom {
    pub nodes: Vec<Node>,
    pub root: Option<usize>,
    pub ids: HashMap<String, usize>,
}

impl Dom {
    pub fn node(&self, i: usize) -> Option<&Node> {
        self.nodes.get(i)
    }

    /// The element a local reference (`#id`) names.
    pub fn by_ref(&self, r: &str) -> Option<usize> {
        let id = r.trim().strip_prefix('#')?;
        self.ids.get(id).copied()
    }
}

fn node_of(e: &BytesStart) -> Node {
    let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
    let attrs = e
        .attributes()
        .flatten()
        .filter_map(|a| {
            let k = String::from_utf8_lossy(a.key.as_ref()).into_owned();
            a.normalized_value(XmlVersion::Explicit1_0).ok().map(|v| (k, v.into_owned()))
        })
        .collect();
    Node { name, attrs, kids: Vec::new() }
}

/// Read the whole element tree.
pub(crate) fn build(text: &str) -> Result<Dom> {
    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(false);
    let mut dom = Dom { nodes: Vec::new(), root: None, ids: HashMap::new() };
    let mut open: Vec<usize> = Vec::new();
    loop {
        let ev = reader.read_event().map_err(|e| IoError::Format(format!("SVG: {e}")))?;
        let (node, push) = match ev {
            Event::Start(e) => (node_of(&e), true),
            Event::Empty(e) => (node_of(&e), false),
            Event::End(_) => {
                open.pop();
                continue;
            }
            Event::Text(t) => {
                if let Ok(s) = t.xml10_content() {
                    add_text(&mut dom, &open, &s);
                }
                continue;
            }
            Event::CData(t) => {
                if let Ok(s) = t.decode() {
                    add_text(&mut dom, &open, &s);
                }
                continue;
            }
            Event::GeneralRef(r) => {
                let resolved = match r.resolve_char_ref() {
                    Ok(Some(c)) => Some(c.to_string()),
                    _ => r.decode().ok().and_then(|n| quick_xml::escape::resolve_predefined_entity(&n)).map(str::to_string),
                };
                if let Some(s) = resolved {
                    add_text(&mut dom, &open, &s);
                }
                continue;
            }
            Event::Eof => break,
            _ => continue,
        };
        if dom.nodes.len() >= MAX_NODES {
            return Err(IoError::Format(format!("SVG: more than {MAX_NODES} elements")));
        }
        let i = dom.nodes.len();
        if let Some(id) = node.attr("id") {
            dom.ids.entry(id.to_string()).or_insert(i);
        }
        dom.nodes.push(node);
        match open.last().and_then(|p| dom.nodes.get_mut(*p)) {
            Some(parent) => parent.kids.push(Kid::El(i)),
            None if dom.root.is_none() => dom.root = Some(i),
            None => {}
        }
        if push {
            if open.len() >= MAX_DEPTH {
                return Err(IoError::Format(format!("SVG: elements nested more than {MAX_DEPTH} deep")));
            }
            open.push(i);
        }
    }
    Ok(dom)
}

/// Text inside an element: kept whole in text and style elements, elsewhere only when it is
/// more than whitespace (which nothing reads, but costs nothing).
fn add_text(dom: &mut Dom, open: &[usize], s: &str) {
    let Some(parent) = open.last().and_then(|p| dom.nodes.get_mut(*p)) else { return };
    if !TEXT_HOLDERS.contains(&parent.name.as_str()) && s.trim().is_empty() {
        return;
    }
    match parent.kids.last_mut() {
        Some(Kid::Text(t)) => t.push_str(s),
        _ => parent.kids.push(Kid::Text(s.to_string())),
    }
}

// --- CSS ------------------------------------------------------------------------------------

/// Rules from `<style>` elements with simple selectors: `tag`, `.class`, `#id`, `*` and
/// compounds of them (`rect.cut`), in comma lists. Rules with combinators, pseudo-classes or
/// attribute selectors, and at-rules (`@media`, `@font-face`), are left out.
#[derive(Default)]
pub(crate) struct Css {
    rules: Vec<Rule>,
}

struct Rule {
    tag: Option<String>,
    id: Option<String>,
    classes: Vec<String>,
    /// (ids, classes, tags), compared as a number.
    spec: u32,
    order: usize,
    decls: Vec<(String, String)>,
}

impl Css {
    pub fn add(&mut self, src: &str) {
        let src = strip_comments(src);
        let mut rest = src.as_str();
        while let Some(open) = rest.find('{') {
            let head = rest[..open].trim();
            let Some(close) = block_end(rest, open) else { break };
            let body = &rest[open + 1..close];
            rest = &rest[close + 1..];
            if head.starts_with('@') {
                continue;
            }
            let decls = declarations(body);
            for sel in head.split(',') {
                if self.rules.len() >= MAX_RULES {
                    return;
                }
                if let Some((tag, id, classes)) = selector(sel.trim()) {
                    let spec = (u32::from(id.is_some()) << 16) | ((classes.len().min(255) as u32) << 8) | u32::from(tag.is_some());
                    let order = self.rules.len();
                    self.rules.push(Rule { tag, id, classes, spec, order, decls: decls.clone() });
                }
            }
        }
    }

    /// The declarations of the rules matching `n`, lowest precedence first.
    pub fn matching<'a>(&'a self, n: &Node) -> Vec<&'a (String, String)> {
        if self.rules.is_empty() {
            return Vec::new();
        }
        let id = n.attr("id");
        let classes: Vec<&str> = n.attr("class").map(|c| c.split_whitespace().collect()).unwrap_or_default();
        let mut hits: Vec<&Rule> = self
            .rules
            .iter()
            .filter(|r| {
                r.tag.as_deref().is_none_or(|t| t == n.name)
                    && r.id.as_deref().is_none_or(|i| Some(i) == id)
                    && r.classes.iter().all(|c| classes.contains(&c.as_str()))
            })
            .collect();
        hits.sort_by_key(|r| (r.spec, r.order));
        hits.into_iter().flat_map(|r| r.decls.iter()).collect()
    }
}

fn strip_comments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("/*") {
        out.push_str(&rest[..i]);
        match rest[i + 2..].find("*/") {
            Some(j) => rest = &rest[i + 2 + j + 2..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// The `}` closing the block opened at `open`, counting nested blocks (an at-rule's).
fn block_end(s: &str, open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (i, c) in s.char_indices().skip_while(|(i, _)| *i < open) {
        match c {
            '{' => depth += 1,
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// A compound selector's tag, id and classes; `None` when it is not a simple one.
type Selector = (Option<String>, Option<String>, Vec<String>);

fn selector(s: &str) -> Option<Selector> {
    if s.is_empty() || s.contains(|c: char| c.is_whitespace() || matches!(c, '>' | '+' | '~' | ':' | '[' | '(')) {
        return None;
    }
    let ident = |c: char| c.is_alphanumeric() || matches!(c, '-' | '_') || !c.is_ascii();
    let mut tag = None;
    let mut id = None;
    let mut classes = Vec::new();
    let mut rest = s;
    if let Some(r) = rest.strip_prefix('*') {
        rest = r;
    } else {
        let end = rest.find(|c: char| !ident(c)).unwrap_or(rest.len());
        if end > 0 {
            tag = Some(rest[..end].to_string());
            rest = &rest[end..];
        }
    }
    while let Some(kind) = rest.chars().next() {
        let body = &rest[1..];
        let end = body.find(|c: char| !ident(c)).unwrap_or(body.len());
        if end == 0 {
            return None;
        }
        let name = body[..end].to_string();
        match kind {
            '.' => classes.push(name),
            '#' => id = Some(name),
            _ => return None,
        }
        rest = &body[end..];
    }
    Some((tag, id, classes))
}

/// `name: value; …` (a `style` attribute or a rule's body), names in lower case and
/// `!important` dropped.
pub(crate) fn declarations(s: &str) -> Vec<(String, String)> {
    s.split(';')
        .filter_map(|d| {
            let (k, v) = d.split_once(':')?;
            let k = k.trim().to_ascii_lowercase();
            let v = v.trim();
            let v = v.strip_suffix("!important").unwrap_or(v).trim();
            (!k.is_empty() && !v.is_empty()).then(|| (k, v.to_string()))
        })
        .collect()
}

// --- computed style ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Paint {
    None,
    Color(Rgb),
    /// A paint server (a gradient), with the colour to use when it can't be found.
    Url(String, Option<Rgb>),
    Current,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Anchor {
    Start,
    Middle,
    End,
}

/// The properties the reader uses, as they apply to one element.
#[derive(Clone, Debug)]
pub(crate) struct Style {
    /// `fill`, when the file sets it (inherited); unset fills are not read as hatches.
    pub fill: Option<Paint>,
    /// `color`, for `currentColor` (inherited).
    pub color: Rgb,
    /// `font-size` in user units (inherited).
    pub font_size: f64,
    pub anchor: Anchor,
    /// `display: none` (not inherited: a hidden element is skipped whole).
    pub hidden: bool,
    /// `stop-color` of a gradient stop.
    pub stop: Option<Rgb>,
}

impl Default for Style {
    fn default() -> Self {
        Style { fill: None, color: Rgb(0, 0, 0), font_size: 16.0, anchor: Anchor::Start, hidden: false, stop: None }
    }
}

/// Properties read from presentation attributes as well as from CSS.
const PROPS: &[&str] = &["fill", "color", "font-size", "text-anchor", "display", "stop-color"];

impl Style {
    /// The style of `n` inside `parent`: presentation attributes, then matching CSS rules (by
    /// specificity, then order), then the `style` attribute.
    pub fn of(n: &Node, parent: &Style, css: &Css) -> Style {
        let mut s = Style { hidden: false, stop: None, ..parent.clone() };
        for p in PROPS {
            if let Some(v) = n.attr(p) {
                s.apply(p, v, parent);
            }
        }
        for (k, v) in css.matching(n) {
            s.apply(k, v, parent);
        }
        if let Some(st) = n.attr("style") {
            for (k, v) in declarations(st) {
                s.apply(&k, &v, parent);
            }
        }
        s
    }

    fn apply(&mut self, k: &str, v: &str, parent: &Style) {
        let v = v.trim();
        if v.eq_ignore_ascii_case("inherit") {
            return;
        }
        match k {
            "fill" => {
                if let Some(p) = paint(v) {
                    self.fill = Some(p);
                }
            }
            "color" => {
                if let Some(c) = color(v) {
                    self.color = c;
                }
            }
            "font-size" => {
                if let Some(f) = font_size(v, parent.font_size) {
                    self.font_size = f;
                }
            }
            "text-anchor" => {
                self.anchor = match v {
                    "middle" => Anchor::Middle,
                    "end" => Anchor::End,
                    _ => Anchor::Start,
                }
            }
            "display" => self.hidden = v == "none",
            "stop-color" => self.stop = if v == "currentColor" { Some(self.color) } else { color(v) },
            _ => {}
        }
    }

    /// The fill colour when the fill is a plain colour.
    pub fn fill_rgb(&self) -> Option<Rgb> {
        match &self.fill {
            Some(Paint::Color(c)) => Some(*c),
            Some(Paint::Current) => Some(self.color),
            _ => None,
        }
    }
}

pub(crate) fn paint(v: &str) -> Option<Paint> {
    let v = v.trim();
    if v == "none" {
        return Some(Paint::None);
    }
    if v == "currentColor" {
        return Some(Paint::Current);
    }
    if let Some(rest) = v.strip_prefix("url(") {
        let close = rest.find(')')?;
        let r = rest[..close].trim().trim_matches(|c| c == '"' || c == '\'');
        let fallback = color(rest[close + 1..].trim());
        return Some(Paint::Url(r.to_string(), fallback));
    }
    color(v).map(Paint::Color)
}

/// A font size in user units: a number, px, pt, pc, mm, cm, in, em or a percentage of the
/// parent's, or a CSS size keyword.
fn font_size(v: &str, parent: f64) -> Option<f64> {
    let keyword = match v {
        "xx-small" => Some(9.0),
        "x-small" => Some(10.0),
        "small" => Some(13.0),
        "medium" => Some(16.0),
        "large" => Some(18.0),
        "x-large" => Some(24.0),
        "xx-large" => Some(32.0),
        "smaller" => Some(parent / 1.2),
        "larger" => Some(parent * 1.2),
        _ => None,
    };
    if keyword.is_some() {
        return keyword;
    }
    let unit = ["px", "pt", "pc", "mm", "cm", "in", "em", "ex", "%"].into_iter().find(|u| v.ends_with(u)).unwrap_or("");
    let n: f64 = v[..v.len() - unit.len()].trim().parse().ok().filter(|x: &f64| x.is_finite() && *x > 0.0)?;
    let px = match unit {
        "pt" => n * 96.0 / 72.0,
        "pc" => n * 16.0,
        "mm" => n * 96.0 / 25.4,
        "cm" => n * 96.0 / 2.54,
        "in" => n * 96.0,
        "em" => n * parent,
        "ex" => n * parent * 0.5,
        "%" => n * parent / 100.0,
        _ => n,
    };
    px.is_finite().then_some(px)
}

/// A CSS colour: `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`, `rgb()`/`rgba()` with numbers or
/// percentages, or a colour keyword. Alpha is dropped.
pub(crate) fn color(v: &str) -> Option<Rgb> {
    let v = v.trim();
    if let Some(hex) = v.strip_prefix('#') {
        let digit = |i: usize| hex.get(i..=i).and_then(|d| u8::from_str_radix(d, 16).ok());
        let pair = |i: usize| hex.get(i..i + 2).and_then(|d| u8::from_str_radix(d, 16).ok());
        return match hex.len() {
            3 | 4 => Some(Rgb(digit(0)? * 17, digit(1)? * 17, digit(2)? * 17)),
            6 | 8 => Some(Rgb(pair(0)?, pair(2)?, pair(4)?)),
            _ => None,
        };
    }
    let lower = v.to_ascii_lowercase();
    if let Some(args) = lower.strip_prefix("rgba(").or_else(|| lower.strip_prefix("rgb(")) {
        let args = args.strip_suffix(')')?;
        let parts: Vec<&str> = args.split(|c: char| c == ',' || c == '/' || c.is_whitespace()).filter(|p| !p.is_empty()).collect();
        let channel = |p: &str| -> Option<u8> {
            let x = match p.strip_suffix('%') {
                Some(pc) => pc.parse::<f64>().ok()? / 100.0 * 255.0,
                None => p.parse::<f64>().ok()?,
            };
            x.is_finite().then(|| x.round().clamp(0.0, 255.0) as u8)
        };
        return Some(Rgb(channel(parts.first()?)?, channel(parts.get(1)?)?, channel(parts.get(2)?)?));
    }
    NAMED.iter().find(|(n, _)| *n == lower).map(|(_, c)| Rgb::from_u32(*c))
}

const NAMED: &[(&str, u32)] = &[
    ("black", 0x000000),
    ("silver", 0xc0c0c0),
    ("gray", 0x808080),
    ("grey", 0x808080),
    ("white", 0xffffff),
    ("maroon", 0x800000),
    ("red", 0xff0000),
    ("purple", 0x800080),
    ("fuchsia", 0xff00ff),
    ("magenta", 0xff00ff),
    ("green", 0x008000),
    ("lime", 0x00ff00),
    ("olive", 0x808000),
    ("yellow", 0xffff00),
    ("navy", 0x000080),
    ("blue", 0x0000ff),
    ("teal", 0x008080),
    ("aqua", 0x00ffff),
    ("cyan", 0x00ffff),
    ("orange", 0xffa500),
    ("brown", 0xa52a2a),
    ("pink", 0xffc0cb),
    ("gold", 0xffd700),
    ("violet", 0xee82ee),
    ("indigo", 0x4b0082),
    ("darkgray", 0xa9a9a9),
    ("darkgrey", 0xa9a9a9),
    ("lightgray", 0xd3d3d3),
    ("lightgrey", 0xd3d3d3),
    ("dimgray", 0x696969),
    ("dimgrey", 0x696969),
    ("darkred", 0x8b0000),
    ("darkgreen", 0x006400),
    ("darkblue", 0x00008b),
    ("lightblue", 0xadd8e6),
    ("lightgreen", 0x90ee90),
    ("skyblue", 0x87ceeb),
    ("steelblue", 0x4682b4),
    ("tomato", 0xff6347),
    ("crimson", 0xdc143c),
    ("coral", 0xff7f50),
    ("salmon", 0xfa8072),
    ("khaki", 0xf0e68c),
    ("tan", 0xd2b48c),
    ("beige", 0xf5f5dc),
    ("ivory", 0xfffff0),
    ("wheat", 0xf5deb3),
    ("chocolate", 0xd2691e),
    ("orchid", 0xda70d6),
    ("plum", 0xdda0dd),
    ("turquoise", 0x40e0d0),
    ("forestgreen", 0x228b22),
    ("seagreen", 0x2e8b57),
    ("royalblue", 0x4169e1),
    ("slategray", 0x708090),
    ("slategrey", 0x708090),
    ("whitesmoke", 0xf5f5f5),
    ("gainsboro", 0xdcdcdc),
];

// --- data URLs --------------------------------------------------------------------------------

/// The media type and bytes of a base64 `data:` URL.
pub(crate) fn data_url(href: &str, max: usize) -> Option<(String, Vec<u8>)> {
    let rest = href.trim().strip_prefix("data:")?;
    let (head, payload) = rest.split_once(',')?;
    let head = head.to_ascii_lowercase();
    let (mime, base64) = match head.strip_suffix(";base64") {
        Some(m) => (m, true),
        None => (head.as_str(), false),
    };
    if !base64 || payload.len() / 4 * 3 > max {
        return None;
    }
    let mime = mime.split(';').next().unwrap_or("").trim().to_string();
    Some((mime, base64_decode(payload)?))
}

fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let mut acc = 0u32;
    let mut bits = 0u32;
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            b' ' | b'\t' | b'\r' | b'\n' => continue,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours() {
        assert_eq!(color("#f80"), Some(Rgb(255, 136, 0)));
        assert_eq!(color("#00ff0080"), Some(Rgb(0, 255, 0)));
        assert_eq!(color("rgb(10, 20, 30)"), Some(Rgb(10, 20, 30)));
        assert_eq!(color("rgb(100%,0%,50%)"), Some(Rgb(255, 0, 128)));
        assert_eq!(color("Navy"), Some(Rgb(0, 0, 128)));
        assert_eq!(color("#ggg"), None);
        assert_eq!(color("#ééé"), None);
        assert_eq!(color("rgb(1,2)"), None);
    }

    #[test]
    fn selectors_and_cascade() {
        let mut css = Css::default();
        css.add("/* x */ rect { fill: red } .a{fill:#00f} rect.a { fill: lime } #r { fill: black } g rect { fill: pink } @media print { rect { fill: gray } }");
        let n = Node { name: "rect".into(), attrs: vec![("class".into(), "a b".into())], kids: Vec::new() };
        let s = Style::of(&n, &Style::default(), &css);
        assert_eq!(s.fill, Some(Paint::Color(Rgb(0, 255, 0))));
        let n = Node { name: "rect".into(), attrs: vec![("class".into(), "a".into()), ("id".into(), "r".into())], kids: Vec::new() };
        assert_eq!(Style::of(&n, &Style::default(), &css).fill, Some(Paint::Color(Rgb(0, 0, 0))));
        // The style attribute wins over every rule; a presentation attribute loses to them.
        let n = Node { name: "rect".into(), attrs: vec![("fill".into(), "blue".into()), ("style".into(), "fill:yellow".into())], kids: Vec::new() };
        assert_eq!(Style::of(&n, &Style::default(), &css).fill, Some(Paint::Color(Rgb(255, 255, 0))));
        let n = Node { name: "rect".into(), attrs: vec![("fill".into(), "blue".into())], kids: Vec::new() };
        assert_eq!(Style::of(&n, &Style::default(), &css).fill, Some(Paint::Color(Rgb(255, 0, 0))));
    }

    #[test]
    fn data_urls() {
        let (mime, bytes) = data_url("data:image/png;base64,aGVs\nbG8=", 1 << 20).unwrap();
        assert_eq!((mime.as_str(), bytes.as_slice()), ("image/png", b"hello".as_slice()));
        assert!(data_url("data:image/png;base64,aGVsbG8=", 2).is_none());
        assert!(data_url("data:text/plain,hello", 1 << 20).is_none());
        assert!(data_url("data:image/png;base64,a*b", 1 << 20).is_none());
    }

    #[test]
    fn font_sizes() {
        assert_eq!(font_size("12", 16.0), Some(12.0));
        assert_eq!(font_size("2em", 10.0), Some(20.0));
        assert_eq!(font_size("150%", 10.0), Some(15.0));
        assert_eq!(font_size("12pt", 16.0), Some(16.0));
        assert_eq!(font_size("-3", 16.0), None);
        assert_eq!(font_size("big", 16.0), None);
    }
}
