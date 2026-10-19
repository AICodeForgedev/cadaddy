//! DCL, the dialog control language of AutoLISP dialogs (`load_dialog`, `new_dialog`,
//! `start_dialog`…), as the public AutoLISP and DCL documentation describes it: a file of
//! definitions, `name : prototype { attribute = value; : tile { … } subtile; }`. A definition
//! names a tile made from a prototype — a built-in tile type (`dialog`, `row`, `button`,
//! `edit_box`…) or another definition — with attributes of its own and tiles inside.
//!
//! The standard clusters (`ok_only`, `ok_cancel`, `ok_cancel_help`, `errtile`, `spacer`…) are
//! CADaddy's own definitions of what the documentation says they hold; nothing is taken from
//! another program's files.

use std::collections::BTreeMap;

/// Definitions nested deeper than this (prototypes of prototypes, tiles in tiles) are an error: a
/// hostile file can't recurse without end.
pub const MAX_DEPTH: usize = 32;
/// Tiles in one dialog, at most.
pub const MAX_TILES: usize = 5000;
/// A DCL file's size, at most.
pub const MAX_SOURCE: usize = 4 << 20;

/// The built-in tile types (DCL reference: predefined tiles and clusters).
pub const TILE_TYPES: &[&str] = &[
    "dialog",
    "row",
    "column",
    "boxed_row",
    "boxed_column",
    "radio_row",
    "radio_column",
    "boxed_radio_row",
    "boxed_radio_column",
    "concatenation",
    "paragraph",
    "button",
    "edit_box",
    "image",
    "image_button",
    "list_box",
    "popup_list",
    "radio_button",
    "slider",
    "spacer",
    "text",
    "text_part",
    "toggle",
    "errtile",
    "spacer_0",
    "spacer_1",
];

/// A tile: its type (the built-in it comes down to), attributes and the tiles inside it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tile {
    pub kind: String,
    /// Attribute values as written (strings unquoted), by lower-case name.
    pub attrs: BTreeMap<String, String>,
    pub children: Vec<Tile>,
}

impl Tile {
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.get(name).map(String::as_str)
    }
    pub fn key(&self) -> Option<&str> {
        self.attr("key")
    }
    /// A `true`/`false` attribute.
    pub fn flag(&self, name: &str) -> bool {
        self.attr(name).is_some_and(|v| v.eq_ignore_ascii_case("true"))
    }
    /// Every tile in this one, depth first, itself first.
    pub fn walk(&self) -> Vec<&Tile> {
        let mut out = vec![self];
        for c in &self.children {
            out.extend(c.walk());
        }
        out
    }
}

/// A definition as written: its prototype, attributes and tiles (resolved when a dialog is made).
#[derive(Clone, Debug, PartialEq)]
struct Def {
    proto: String,
    attrs: BTreeMap<String, String>,
    children: Vec<Item>,
}

/// A tile inside a definition: `: proto { … }` or a reference to a definition, `ok_cancel;`.
#[derive(Clone, Debug, PartialEq)]
enum Item {
    Inline(Def),
    Named(String),
}

/// The definitions of a loaded DCL file, with the standard ones.
#[derive(Clone, Debug, Default)]
pub struct DclFile {
    defs: BTreeMap<String, Def>,
}

/// The standard clusters, defined from the DCL documentation's description of each.
const STANDARD: &str = r#"
spacer_0 : spacer { width = 0; height = 0; }
spacer_1 : spacer { width = 1; height = 1; }
errtile : text { key = "error"; label = ""; is_error_tile = true; }
ok_button : button { key = "accept"; label = "OK"; is_default = true; }
cancel_button : button { key = "cancel"; label = "Cancel"; is_cancel = true; }
help_button : button { key = "help"; label = "Help..."; }
info_button : button { key = "info"; label = "Info..."; }
ok_only : row { fixed_width = true; alignment = centered; ok_button; }
ok_cancel : row { fixed_width = true; alignment = centered; ok_button; cancel_button; }
ok_cancel_help : row { fixed_width = true; alignment = centered; ok_button; cancel_button; help_button; }
ok_cancel_help_info : row { fixed_width = true; alignment = centered; ok_button; cancel_button; help_button; info_button; }
ok_cancel_help_errtile : column { ok_cancel_help; errtile; }
default_button : button { is_default = true; }
"#;

// ---------------- lexing ----------------

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Word(String),
    Str(String),
    Colon,
    Open,
    Close,
    Eq,
    Semi,
    /// `@include "file"`.
    Include(String),
}

fn lex(src: &str) -> Result<Vec<Tok>, String> {
    let mut out = Vec::new();
    let mut it = src.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            c if c.is_whitespace() => {}
            '/' if it.peek() == Some(&'/') => {
                for d in it.by_ref() {
                    if d == '\n' {
                        break;
                    }
                }
            }
            '/' if it.peek() == Some(&'*') => {
                it.next();
                let mut prev = ' ';
                for d in it.by_ref() {
                    if prev == '*' && d == '/' {
                        break;
                    }
                    prev = d;
                }
            }
            ':' => out.push(Tok::Colon),
            '{' => out.push(Tok::Open),
            '}' => out.push(Tok::Close),
            '=' => out.push(Tok::Eq),
            ';' => out.push(Tok::Semi),
            '"' => {
                let mut s = String::new();
                loop {
                    match it.next() {
                        Some('\\') => match it.next() {
                            Some('n') => s.push('\n'),
                            Some('t') => s.push('\t'),
                            Some(d) => s.push(d),
                            None => return Err("unterminated string".into()),
                        },
                        Some('"') => break,
                        Some(d) => s.push(d),
                        None => return Err("unterminated string".into()),
                    }
                }
                out.push(Tok::Str(s));
            }
            '@' => {
                let mut w = String::new();
                while let Some(&d) = it.peek() {
                    if d.is_alphanumeric() || d == '_' {
                        w.push(d);
                        it.next();
                    } else {
                        break;
                    }
                }
                if !w.eq_ignore_ascii_case("include") {
                    return Err(format!("unknown directive @{w}"));
                }
                // The file name that follows.
                while it.peek().is_some_and(|d| d.is_whitespace()) {
                    it.next();
                }
                if it.next() != Some('"') {
                    return Err("@include needs a quoted file name".into());
                }
                let name: String = it.by_ref().take_while(|d| *d != '"').collect();
                out.push(Tok::Include(name));
            }
            c if c.is_alphanumeric() || c == '_' || c == '-' || c == '.' || c == '$' => {
                let mut w = String::from(c);
                while let Some(&d) = it.peek() {
                    if d.is_alphanumeric() || matches!(d, '_' | '-' | '.' | '$') {
                        w.push(d);
                        it.next();
                    } else {
                        break;
                    }
                }
                out.push(Tok::Word(w));
            }
            other => return Err(format!("unexpected character {other:?}")),
        }
    }
    Ok(out)
}

// ---------------- parsing ----------------

struct Parser {
    toks: Vec<Tok>,
    at: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.at)
    }
    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.at).cloned();
        self.at += 1;
        t
    }
    fn expect(&mut self, t: &Tok, what: &str) -> Result<(), String> {
        match self.next() {
            Some(ref got) if got == t => Ok(()),
            got => Err(format!("expected {what}, found {got:?}")),
        }
    }
    fn word(&mut self, what: &str) -> Result<String, String> {
        match self.next() {
            Some(Tok::Word(w)) => Ok(w),
            got => Err(format!("expected {what}, found {got:?}")),
        }
    }

    /// `{ attr = value; : proto { … } name; … }`, the opening brace already taken.
    fn body(&mut self, proto: String, depth: usize) -> Result<Def, String> {
        if depth > MAX_DEPTH {
            return Err("tiles nested too deep".into());
        }
        let mut def = Def { proto, attrs: BTreeMap::new(), children: Vec::new() };
        loop {
            match self.next() {
                Some(Tok::Close) => return Ok(def),
                Some(Tok::Colon) => {
                    let p = self.word("a tile type")?;
                    self.expect(&Tok::Open, "{")?;
                    def.children.push(Item::Inline(self.body(p.to_ascii_lowercase(), depth + 1)?));
                    // An optional `;` after a nested tile.
                    if self.peek() == Some(&Tok::Semi) {
                        self.next();
                    }
                }
                Some(Tok::Word(w)) => match self.peek() {
                    Some(Tok::Eq) => {
                        self.next();
                        let v = match self.next() {
                            Some(Tok::Str(s) | Tok::Word(s)) => s,
                            got => return Err(format!("expected the value of {w}, found {got:?}")),
                        };
                        self.expect(&Tok::Semi, ";")?;
                        def.attrs.insert(w.to_ascii_lowercase(), v);
                    }
                    Some(Tok::Semi) => {
                        self.next();
                        def.children.push(Item::Named(w.to_ascii_lowercase()));
                    }
                    got => return Err(format!("expected = or ; after {w}, found {got:?}")),
                },
                got => return Err(format!("unexpected {got:?} in a tile")),
            }
        }
    }
}

/// The definitions of DCL source `src`; `include` reads an `@include`d file (`None`: not found).
pub fn parse(src: &str, include: &mut dyn FnMut(&str) -> Option<String>) -> Result<DclFile, String> {
    parse_into(src, include, &mut DclFile::standard(), 0)
}

fn parse_into(src: &str, include: &mut dyn FnMut(&str) -> Option<String>, file: &mut DclFile, depth: usize) -> Result<DclFile, String> {
    if src.len() > MAX_SOURCE {
        return Err("the DCL file is too large".into());
    }
    if depth > 8 {
        return Err("@include nested too deep".into());
    }
    let mut p = Parser { toks: lex(src)?, at: 0 };
    while let Some(t) = p.next() {
        match t {
            Tok::Include(name) => {
                let text = include(&name).ok_or_else(|| format!("@include \"{name}\": not found"))?;
                *file = parse_into(&text, include, file, depth + 1)?;
            }
            Tok::Word(name) => {
                p.expect(&Tok::Colon, ":")?;
                let proto = p.word("a prototype")?;
                p.expect(&Tok::Open, "{")?;
                let def = p.body(proto.to_ascii_lowercase(), 0)?;
                file.defs.insert(name.to_ascii_lowercase(), def);
                if p.peek() == Some(&Tok::Semi) {
                    p.next();
                }
            }
            got => return Err(format!("expected a definition, found {got:?}")),
        }
    }
    Ok(file.clone())
}

impl DclFile {
    /// The standard clusters alone.
    pub fn standard() -> DclFile {
        let mut f = DclFile::default();
        let mut p = Parser { toks: lex(STANDARD).unwrap_or_default(), at: 0 };
        while let Some(Tok::Word(name)) = p.next() {
            let (Ok(()), Ok(proto), Ok(())) = (p.expect(&Tok::Colon, ":"), p.word("prototype"), p.expect(&Tok::Open, "{")) else { break };
            let Ok(def) = p.body(proto, 0) else { break };
            f.defs.insert(name, def);
        }
        f
    }

    /// The dialogs the file defines (prototype `dialog`).
    pub fn dialogs(&self) -> Vec<String> {
        self.defs.iter().filter(|(_, d)| self.base_kind(&d.proto, 0).as_deref() == Some("dialog")).map(|(n, _)| n.clone()).collect()
    }

    fn base_kind(&self, proto: &str, depth: usize) -> Option<String> {
        if TILE_TYPES.contains(&proto) {
            return Some(proto.to_string());
        }
        if depth > MAX_DEPTH {
            return None;
        }
        self.base_kind(&self.defs.get(proto)?.proto, depth + 1)
    }

    /// Dialog `name` made into tiles: prototypes resolved (a tile's own attributes over its
    /// prototype's, its prototype's tiles first).
    pub fn dialog(&self, name: &str) -> Result<Tile, String> {
        let def = self.defs.get(&name.to_ascii_lowercase()).ok_or_else(|| format!("no dialog \"{name}\" in the DCL file"))?;
        let mut count = 0;
        let tile = self.resolve(def, 0, &mut count)?;
        if tile.kind != "dialog" {
            return Err(format!("\"{name}\" is a {}, not a dialog", tile.kind));
        }
        Ok(tile)
    }

    fn resolve(&self, def: &Def, depth: usize, count: &mut usize) -> Result<Tile, String> {
        if depth > MAX_DEPTH {
            return Err("prototypes nested too deep (or a definition uses itself)".into());
        }
        *count += 1;
        if *count > MAX_TILES {
            return Err("too many tiles".into());
        }
        let mut tile = if TILE_TYPES.contains(&def.proto.as_str()) {
            Tile { kind: def.proto.clone(), ..Tile::default() }
        } else {
            let proto = self.defs.get(&def.proto).ok_or_else(|| format!("unknown tile type \"{}\"", def.proto))?;
            self.resolve(proto, depth + 1, count)?
        };
        for (k, v) in &def.attrs {
            tile.attrs.insert(k.clone(), v.clone());
        }
        for item in &def.children {
            let child = match item {
                Item::Inline(d) => self.resolve(d, depth + 1, count)?,
                Item::Named(n) => {
                    let d = self.defs.get(n).ok_or_else(|| format!("unknown tile \"{n}\""))?;
                    self.resolve(d, depth + 1, count)?
                }
            };
            tile.children.push(child);
        }
        Ok(tile)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = r#"
        // A dialog of every common tile.
        my_button : button { width = 12; }
        demo : dialog {
            label = "Demo";
            : boxed_column {
                label = "Values";
                : edit_box { key = "name"; label = "Name:"; edit_width = 20; value = "first"; }
                : toggle { key = "bold"; label = "Bold"; }
                : popup_list { key = "size"; label = "Size"; }
                : list_box { key = "items"; height = 5; }
            }
            : radio_row { : radio_button { key = "a"; label = "A"; } : radio_button { key = "b"; label = "B"; } }
            : my_button { key = "go"; label = "Go"; }
            : image { key = "pic"; width = 10; height = 4; }
            ok_cancel;
        }
    "#;

    fn no_include(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn a_dialog_with_its_prototypes_resolved() {
        let f = parse(SRC, &mut no_include).unwrap();
        assert_eq!(f.dialogs(), ["demo"]);
        let d = f.dialog("DEMO").unwrap();
        assert_eq!(d.attr("label"), Some("Demo"));
        let keys: Vec<&str> = d.walk().iter().filter_map(|t| t.key()).collect();
        assert_eq!(keys, ["name", "bold", "size", "items", "a", "b", "go", "pic", "accept", "cancel"]);
        let go = d.walk().into_iter().find(|t| t.key() == Some("go")).unwrap();
        assert_eq!((go.kind.as_str(), go.attr("width"), go.attr("label")), ("button", Some("12"), Some("Go")));
        let ok = d.walk().into_iter().find(|t| t.key() == Some("accept")).unwrap();
        assert!(ok.flag("is_default"));
    }

    #[test]
    fn includes_and_errors() {
        let mut inc = |n: &str| (n == "base.dcl").then(|| "shared : button { label = \"Shared\"; }".to_string());
        let f = parse("@include \"base.dcl\"\nd : dialog { : shared { key = \"s\"; } }", &mut inc).unwrap();
        assert_eq!(f.dialog("d").unwrap().children[0].attr("label"), Some("Shared"));
        assert!(parse("@include \"missing.dcl\"", &mut no_include).is_err());
        assert!(parse("d : dialog { label = \"x\" }", &mut no_include).is_err(), "a missing ;");
        assert!(parse("d : dialog { : nothing { } }", &mut no_include).unwrap().dialog("d").is_err(), "an unknown tile type");
        // A definition that uses itself, and deep nesting: errors, not a stack overflow.
        assert!(parse("loop : loop { }\nd : dialog { loop; }", &mut no_include).unwrap().dialog("d").is_err());
        let deep = format!("d : dialog {{ {} {} }}", ": row {".repeat(200), "}".repeat(200));
        assert!(parse(&deep, &mut no_include).is_err());
        assert!(parse("\"unterminated", &mut no_include).is_err());
    }
}
