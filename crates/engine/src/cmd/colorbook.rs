//! Colour books: named colours in pages, picked by book and colour name (the COlorbook option of
//! colour prompts). A colour taken from a book is applied as its true colour.
//!
//! CADaddy has one book of its own, "CADaddy Basics" (colours we chose and named). More books load
//! from files for the rest of the session (`colorbook {action: "load"}`), in either of two formats:
//!
//! - JSON: `{"name": "Site Colors", "description": "…", "pages": [{"name": "Greens",
//!   "colors": [{"name": "Lawn", "rgb": "#4c8a3b"}, …]}, …]}`; `rgb` may also be `[r, g, b]`.
//! - TSV (UTF-8, tab-separated): a line `book<TAB>Site Colors`, then one line per colour
//!   `page name<TAB>colour name<TAB>#rrggbb`. Empty lines and lines starting with `#` are skipped.
//!
//! Book and colour names match in any case. A loaded book replaces one of the same name, except
//! the built-in book.

use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};

use cadaddy_color::Rgb;
use serde_json::{Value, json};

use super::helpers::support_text;
use super::{CommandSpec, bad, str_param};
use crate::{Result, Session};

/// Most books loaded at once, colours in one book, and characters in a name.
const MAX_BOOKS: usize = 64;
const MAX_COLORS: usize = 20_000;
const MAX_NAME: usize = 255;

/// The built-in book's name.
pub const BASICS: &str = "CADaddy Basics";

#[derive(Clone, Debug, PartialEq)]
pub struct Page {
    pub name: String,
    pub colors: Vec<(String, Rgb)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Book {
    pub name: String,
    pub description: String,
    pub pages: Vec<Page>,
}

impl Book {
    /// The colour called `name` (any case).
    pub fn color(&self, name: &str) -> Option<Rgb> {
        let name = name.trim();
        self.pages.iter().flat_map(|p| &p.colors).find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, c)| *c)
    }
    fn count(&self) -> usize {
        self.pages.iter().map(|p| p.colors.len()).sum()
    }
}

/// CADaddy's own book: five pages of six colours each.
fn basics() -> Book {
    let page = |name: &str, colors: &[(&str, u32)]| Page {
        name: name.into(),
        colors: colors.iter().map(|(n, v)| ((*n).to_string(), Rgb((v >> 16) as u8, (v >> 8) as u8, *v as u8))).collect(),
    };
    Book {
        name: BASICS.into(),
        description: "CADaddy's own colours for drawings: neutrals, earth, water, plants and signals.".into(),
        pages: vec![
            page(
                "Neutrals",
                &[("Chalk", 0xF4F1EA), ("Fog", 0xC9CCCF), ("Pewter", 0x8F959B), ("Slate", 0x5B636B), ("Graphite", 0x34393E), ("Ink", 0x16191C)],
            ),
            page(
                "Earth",
                &[("Sand", 0xE2CFA5), ("Clay", 0xC2875A), ("Brick", 0x9E4B32), ("Umber", 0x6B4A2E), ("Ochre", 0xC99A2E), ("Loam", 0x4A3A2B)],
            ),
            page(
                "Water",
                &[("Shallows", 0xB9E1E8), ("Lagoon", 0x5FB3C2), ("Harbor", 0x2E7D99), ("Deep", 0x1D4E73), ("Ice", 0xDDEEF7), ("Storm", 0x46607A)],
            ),
            page(
                "Plants",
                &[("Sprout", 0xB8DC7C), ("Meadow", 0x7FB24A), ("Fern", 0x4F8A3A), ("Pine", 0x2F5E3A), ("Moss", 0x6E7B3A), ("Sage", 0x9DB39A)],
            ),
            page(
                "Signals",
                &[("Alert", 0xD7322F), ("Caution", 0xF2B01E), ("Go", 0x2E9E4F), ("Info", 0x2F6FD6), ("Notice", 0xE8762D), ("Hold", 0x8B4FC4)],
            ),
        ],
    }
}

fn store() -> MutexGuard<'static, Vec<Book>> {
    static BOOKS: OnceLock<Mutex<Vec<Book>>> = OnceLock::new();
    BOOKS.get_or_init(|| Mutex::new(Vec::new())).lock().unwrap_or_else(PoisonError::into_inner)
}

/// Every book: the built-in one, then those loaded.
pub fn books() -> Vec<Book> {
    let mut v = vec![basics()];
    v.extend(store().iter().cloned());
    v
}

/// Book `name` (any case).
pub fn book(name: &str) -> Option<Book> {
    let name = name.trim();
    books().into_iter().find(|b| b.name.eq_ignore_ascii_case(name))
}

/// Colour `color` of book `book`, or a message saying which isn't there.
pub fn lookup(book_name: &str, color: &str) -> std::result::Result<Rgb, String> {
    let b = book(book_name).ok_or_else(|| format!("Color book \"{}\" not found.", book_name.trim()))?;
    b.color(color).ok_or_else(|| format!("Color \"{}\" not found in color book \"{}\".", color.trim(), b.name))
}

fn name(v: &str, what: &str) -> std::result::Result<String, String> {
    let v = v.trim();
    if v.is_empty() || v.chars().count() > MAX_NAME || v.chars().any(char::is_control) {
        return Err(format!("{what} must be 1 to {MAX_NAME} characters"));
    }
    Ok(v.to_string())
}

fn hex_rgb(v: &str) -> Option<Rgb> {
    let h = v.trim().strip_prefix('#')?;
    if h.len() != 6 || !h.is_ascii() {
        return None;
    }
    let n = u32::from_str_radix(h, 16).ok()?;
    Some(Rgb((n >> 16) as u8, (n >> 8) as u8, n as u8))
}

fn json_rgb(v: &Value) -> Option<Rgb> {
    match v {
        Value::String(s) => hex_rgb(s),
        Value::Array(a) if a.len() == 3 => {
            let c: Option<Vec<u8>> = a.iter().map(|x| x.as_u64().and_then(|n| u8::try_from(n).ok())).collect();
            c.and_then(|c| Some(Rgb(*c.first()?, *c.get(1)?, *c.get(2)?)))
        }
        _ => None,
    }
}

/// Read a book from `text` (JSON or TSV, see the module documentation).
pub fn parse(text: &str) -> std::result::Result<Book, String> {
    let t = text.trim_start_matches('\u{feff}').trim();
    let book = if t.starts_with('{') { parse_json(t)? } else { parse_tsv(t)? };
    if book.count() == 0 {
        return Err("the book has no colours".into());
    }
    if book.count() > MAX_COLORS {
        return Err(format!("the book has more than {MAX_COLORS} colours"));
    }
    Ok(book)
}

fn parse_json(t: &str) -> std::result::Result<Book, String> {
    let v: Value = serde_json::from_str(t).map_err(|e| format!("not a colour book: {e}"))?;
    let book_name = name(v.get("name").and_then(Value::as_str).unwrap_or_default(), "the book's `name`")?;
    let description = v.get("description").and_then(Value::as_str).unwrap_or_default().chars().take(1000).collect();
    let mut pages = Vec::new();
    let mut total = 0usize;
    for p in v.get("pages").and_then(Value::as_array).ok_or("the book needs `pages`")? {
        let page = name(p.get("name").and_then(Value::as_str).unwrap_or_default(), "each page's `name`")?;
        let mut colors = Vec::new();
        for c in p.get("colors").and_then(Value::as_array).ok_or_else(|| format!("page \"{page}\" needs `colors`"))? {
            total += 1;
            if total > MAX_COLORS {
                return Err(format!("the book has more than {MAX_COLORS} colours"));
            }
            let n = name(c.get("name").and_then(Value::as_str).unwrap_or_default(), "each colour's `name`")?;
            let rgb = c.get("rgb").and_then(json_rgb).ok_or_else(|| format!("colour \"{n}\": `rgb` must be \"#rrggbb\" or [r, g, b]"))?;
            colors.push((n, rgb));
        }
        pages.push(Page { name: page, colors });
    }
    Ok(Book { name: book_name, description, pages })
}

fn parse_tsv(t: &str) -> std::result::Result<Book, String> {
    let mut book_name = None;
    let mut pages: Vec<Page> = Vec::new();
    let mut total = 0usize;
    for (i, line) in t.lines().enumerate() {
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let cols: Vec<&str> = line.split('\t').collect();
        match cols.as_slice() {
            [k, n] if k.trim().eq_ignore_ascii_case("book") => book_name = Some(name(n, "the book's name")?),
            [page, color, rgb] => {
                total += 1;
                if total > MAX_COLORS {
                    return Err(format!("the book has more than {MAX_COLORS} colours"));
                }
                let page = name(page, "a page name")?;
                let color = name(color, "a colour name")?;
                let rgb = hex_rgb(rgb).ok_or_else(|| format!("line {}: the colour must be #rrggbb", i + 1))?;
                match pages.iter_mut().find(|p| p.name == page) {
                    Some(p) => p.colors.push((color, rgb)),
                    None => pages.push(Page { name: page, colors: vec![(color, rgb)] }),
                }
            }
            _ => return Err(format!("line {}: expected `book<TAB>name` or `page<TAB>colour<TAB>#rrggbb`", i + 1)),
        }
    }
    Ok(Book { name: book_name.ok_or("the first line must be `book<TAB>name`")?, description: String::new(), pages })
}

/// Load a book for the rest of the session; returns its name.
pub fn load(book: Book) -> std::result::Result<String, String> {
    if book.name.eq_ignore_ascii_case(BASICS) {
        return Err(format!("\"{BASICS}\" is CADaddy's own book and can't be replaced"));
    }
    let mut st = store();
    st.retain(|b| !b.name.eq_ignore_ascii_case(&book.name));
    if st.len() >= MAX_BOOKS {
        return Err(format!("at most {MAX_BOOKS} colour books can be loaded"));
    }
    let n = book.name.clone();
    st.push(book);
    Ok(n)
}

fn hex(c: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", c.0, c.1, c.2)
}

fn book_json(b: &Book, colors: bool) -> Value {
    let pages: Vec<Value> = b
        .pages
        .iter()
        .map(|p| {
            if colors {
                json!({ "name": p.name, "colors": p.colors.iter().map(|(n, c)| json!({ "name": n, "rgb": hex(*c) })).collect::<Vec<_>>() })
            } else {
                json!({ "name": p.name, "count": p.colors.len() })
            }
        })
        .collect();
    json!({ "name": b.name, "description": b.description, "pages": pages })
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("colorbook", "Color Books", run)
            .params("{action?: \"list\" (default: books and their pages) | \"show\" (book: every colour) | \"load\" (file | text: a JSON or TSV book, kept for the session) | \"find\" (book, color → rgb)}")
            .noundo(),
    ]
}

fn run(_s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "colorbook";
    match str_param(p, "action").unwrap_or("list").to_ascii_lowercase().as_str() {
        "list" => Ok(json!({ "books": books().iter().map(|b| book_json(b, false)).collect::<Vec<_>>() })),
        "show" => {
            let n = str_param(p, "book").ok_or_else(|| bad(cmd, "`book` is required"))?;
            let b = book(n).ok_or_else(|| bad(cmd, format!("no colour book \"{n}\"")))?;
            Ok(book_json(&b, true))
        }
        "load" => {
            let (text, source) = support_text(cmd, p, "file", "text")?.ok_or_else(|| bad(cmd, "`file` or `text` is required"))?;
            let b = parse(&text).map_err(|e| bad(cmd, format!("{source}: {e}")))?;
            let count = b.count();
            let name = load(b).map_err(|e| bad(cmd, e))?;
            Ok(json!({ "book": name, "colors": count }))
        }
        "find" => {
            let (b, c) = (str_param(p, "book").unwrap_or_default(), str_param(p, "color").unwrap_or_default());
            let rgb = lookup(b, c).map_err(|e| bad(cmd, e))?;
            Ok(json!({ "rgb": hex(rgb), "color": format!("{},{},{}", rgb.0, rgb.1, rgb.2) }))
        }
        other => Err(bad(cmd, format!("unknown action \"{other}\" (list, show, load or find)"))),
    }
}
