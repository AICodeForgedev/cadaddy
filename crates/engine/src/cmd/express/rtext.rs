//! RTEXT: reactive text — a multiline text showing a text file's contents or the value of a
//! DIESEL expression, brought up to date by REGEN (and RTEXT.REFRESH).
//!
//! An RTEXT is an MTEXT carrying its source in xdata (application `CADADDY_RTEXT`: 1070 the kind,
//! 0 file or 1 DIESEL, then the source in 1000 strings), so it opens as plain text elsewhere.
//!
//! DIESEL is evaluated from the inside out: `$(function,arg,…)` with the arithmetic (`+ - * /`),
//! comparisons (`= < > != <= >=`), `and or xor` (bitwise), `eq`, `if`, `index`, `nth`, `fix`,
//! `strlen`, `substr`, `upper`, `rtos`, `getvar`, `eval` and `edtime`. Quoted text is literal;
//! an error shows `$?` in place of its macro.

use cadaddy_doc::{Common, EntityKind, Handle, MText, RawTag};
use cadaddy_geom::Vec3;
use serde_json::{Value, json};

use super::super::*;
use crate::{Accept, Input, Interactive, Prompt, Result, Session, Step};

pub const RTEXT_APP: &str = "CADADDY_RTEXT";
/// Longest DIESEL result and deepest nesting.
const MAX_OUT: usize = 64 * 1024;
const MAX_DEPTH: usize = 64;
/// Largest file an RTEXT shows.
const MAX_FILE: u64 = 1 << 20;
/// Characters per xdata string.
const CHUNK: usize = 250;

// ---------- DIESEL ----------

/// Evaluate a DIESEL string.
pub fn diesel(s: &Session, text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    eval_into(s, &chars, &mut i, &mut out, 0, false);
    out
}

/// Evaluate from `i`, appending to `out`. Inside a macro (`in_macro`) stops before a top-level
/// `,` or `)`.
fn eval_into(s: &Session, c: &[char], i: &mut usize, out: &mut String, depth: usize, in_macro: bool) {
    while let Some(&ch) = c.get(*i) {
        if out.len() > MAX_OUT {
            return;
        }
        if in_macro && (ch == ',' || ch == ')') {
            return;
        }
        if in_macro && ch == '"' {
            // Quoted: literal up to the closing quote ("" is a quote).
            *i += 1;
            while let Some(&q) = c.get(*i) {
                *i += 1;
                if q == '"' {
                    if c.get(*i) == Some(&'"') {
                        out.push('"');
                        *i += 1;
                        continue;
                    }
                    break;
                }
                out.push(q);
            }
            continue;
        }
        if ch == '$' && c.get(*i + 1) == Some(&'(') {
            *i += 2;
            let v = if depth >= MAX_DEPTH { None } else { macro_call(s, c, i, depth + 1) };
            out.push_str(&v.unwrap_or_else(|| "$?".into()));
            continue;
        }
        out.push(ch);
        *i += 1;
    }
}

/// A macro's arguments (from just after `$(`) through its `)`, then its value.
fn macro_call(s: &Session, c: &[char], i: &mut usize, depth: usize) -> Option<String> {
    let mut args = Vec::new();
    loop {
        let mut a = String::new();
        eval_into(s, c, i, &mut a, depth, true);
        args.push(a);
        match c.get(*i) {
            Some(',') => *i += 1,
            Some(')') => {
                *i += 1;
                break;
            }
            // Unclosed.
            _ => return None,
        }
        if args.len() > 256 {
            return None;
        }
    }
    let name = args.first()?.trim().to_ascii_lowercase();
    function(s, &name, args.get(1..).unwrap_or_default(), depth)
}

fn num(a: &str) -> Option<f64> {
    let v: f64 = a.trim().parse().ok()?;
    v.is_finite().then_some(v)
}

fn int(a: &str) -> Option<i64> {
    num(a).map(|v| v as i64)
}

/// A number as DIESEL shows it: integers without a decimal point.
fn show(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 { format!("{}", v as i64) } else { format!("{v}") }
}

fn function(s: &Session, name: &str, a: &[String], depth: usize) -> Option<String> {
    let arg = |k: usize| a.get(k).map(String::as_str);
    let nums = || a.iter().map(|x| num(x)).collect::<Option<Vec<f64>>>();
    let cmp = |f: fn(&f64, &f64) -> bool| -> Option<String> {
        let (x, y) = (num(arg(0)?)?, num(arg(1)?)?);
        Some(if f(&x, &y) { "1" } else { "0" }.into())
    };
    Some(match name {
        "+" => show(nums()?.iter().sum()),
        "*" => show(nums()?.iter().product()),
        "-" => {
            let v = nums()?;
            let (first, rest) = v.split_first()?;
            show(rest.iter().fold(*first, |acc, x| acc - x))
        }
        "/" => {
            let v = nums()?;
            let (first, rest) = v.split_first()?;
            if rest.contains(&0.0) {
                return None;
            }
            show(rest.iter().fold(*first, |acc, x| acc / x))
        }
        "=" => cmp(|x, y| x == y)?,
        "<" => cmp(|x, y| x < y)?,
        ">" => cmp(|x, y| x > y)?,
        "!=" => cmp(|x, y| x != y)?,
        "<=" => cmp(|x, y| x <= y)?,
        ">=" => cmp(|x, y| x >= y)?,
        "and" | "or" | "xor" => {
            let v = a.iter().map(|x| int(x)).collect::<Option<Vec<i64>>>()?;
            let r = v.iter().copied().reduce(|x, y| match name {
                "and" => x & y,
                "or" => x | y,
                _ => x ^ y,
            })?;
            r.to_string()
        }
        "eq" => (if arg(0)? == arg(1)? { "1" } else { "0" }).into(),
        "if" => {
            let c = int(arg(0)?)?;
            if c != 0 { arg(1)?.to_string() } else { arg(2).unwrap_or("").to_string() }
        }
        "index" => {
            let k = usize::try_from(int(arg(0)?)?).ok()?;
            arg(1)?.split(',').nth(k).unwrap_or("").to_string()
        }
        "nth" => {
            let k = usize::try_from(int(arg(0)?)?).ok()?;
            a.get(k + 1).cloned().unwrap_or_default()
        }
        "fix" => show(num(arg(0)?)?.trunc()),
        "strlen" => arg(0)?.chars().count().to_string(),
        "substr" => {
            let t: Vec<char> = arg(0)?.chars().collect();
            let start = usize::try_from(int(arg(1)?)?).ok()?.max(1) - 1;
            let len = match arg(2) {
                Some(l) => usize::try_from(int(l)?).ok()?,
                None => t.len(),
            };
            t.iter().skip(start).take(len).collect()
        }
        "upper" => arg(0)?.to_uppercase(),
        "rtos" => {
            let v = num(arg(0)?)?;
            let mode = arg(1).and_then(int).unwrap_or(2);
            let prec = arg(2).and_then(int).unwrap_or(4).clamp(0, 16) as usize;
            match mode {
                1 => format!("{v:.prec$E}"),
                _ => format!("{v:.prec$}"),
            }
        }
        "getvar" => getvar(s, arg(0)?.trim())?,
        "eval" => {
            let t: Vec<char> = arg(0)?.chars().collect();
            let mut out = String::new();
            let mut i = 0;
            eval_into(s, &t, &mut i, &mut out, depth + 1, false);
            out
        }
        "edtime" => edtime(num(arg(0)?)?, arg(1)?),
        _ => return None,
    })
}

/// A system variable as text (DATE and CDATE: now).
fn getvar(s: &Session, name: &str) -> Option<String> {
    match name.to_ascii_uppercase().as_str() {
        "DATE" => Some(format!("{:.8}", julian_now()?)),
        "CDATE" => {
            let (y, mo, d, h, mi, sec) = civil(julian_now()?);
            Some(format!("{y:04}{mo:02}{d:02}.{h:02}{mi:02}{:02}", sec as u32))
        }
        n => match crate::sysvars::get(s, n)? {
            Value::String(t) => Some(t),
            Value::Number(v) => Some(v.as_f64().map(show).unwrap_or_else(|| v.to_string())),
            Value::Bool(b) => Some(if b { "1" } else { "0" }.into()),
            Value::Array(p) => Some(p.iter().map(|x| x.as_f64().map(show).unwrap_or_default()).collect::<Vec<_>>().join(",")),
            _ => None,
        },
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn julian_now() -> Option<f64> {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok()?.as_secs_f64();
    Some(2_440_587.5 + secs / 86_400.0)
}

#[cfg(target_arch = "wasm32")]
fn julian_now() -> Option<f64> {
    None
}

/// Year, month, day, hour, minute and second of a Julian date (UTC).
fn civil(jd: f64) -> (i64, u32, u32, u32, u32, f64) {
    let z = (jd + 0.5).floor();
    let f = jd + 0.5 - z;
    let alpha = ((z - 1_867_216.25) / 36_524.25).floor();
    let a = z + 1.0 + alpha - (alpha / 4.0).floor();
    let b = a + 1524.0;
    let c = ((b - 122.1) / 365.25).floor();
    let d = (365.25 * c).floor();
    let e = ((b - d) / 30.6001).floor();
    let day = (b - d - (30.6001 * e).floor()) as u32;
    let month = if e < 14.0 { e - 1.0 } else { e - 13.0 } as u32;
    let year = if month > 2 { c - 4716.0 } else { c - 4715.0 } as i64;
    let secs = f * 86_400.0;
    let h = (secs / 3600.0).floor();
    let mi = ((secs - h * 3600.0) / 60.0).floor();
    (year, month, day, h as u32, mi as u32, secs - h * 3600.0 - mi * 60.0)
}

/// `edtime`: a Julian date through a picture (D DD DDD DDDD, M MO MON MONTH, YY YYYY, H HH, MM,
/// SS, MSEC, AM/PM, A/P; other characters as they are).
fn edtime(jd: f64, picture: &str) -> String {
    const DAYS: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];
    const MONTHS: [&str; 12] =
        ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    let (y, mo, d, h, mi, sec) = civil(jd);
    let weekday = DAYS.get(((jd + 0.5).floor() as i64).rem_euclid(7) as usize).copied().unwrap_or("");
    let month = MONTHS.get(mo.saturating_sub(1) as usize).copied().unwrap_or("");
    let pm = picture.to_ascii_uppercase().contains("AM/PM") || picture.to_ascii_uppercase().contains("A/P");
    let hour = if pm { (h + 11) % 12 + 1 } else { h };
    let tokens: [(&str, String); 19] = [
        ("MONTH", month.into()),
        ("DDDD", weekday.into()),
        ("YYYY", format!("{y:04}")),
        ("MSEC", format!("{:03}", ((sec.fract()) * 1000.0) as u32)),
        ("AM/PM", (if h < 12 { "AM" } else { "PM" }).into()),
        ("MON", month.chars().take(3).collect()),
        ("DDD", weekday.chars().take(3).collect()),
        ("A/P", (if h < 12 { "A" } else { "P" }).into()),
        ("DD", format!("{d:02}")),
        ("MO", format!("{mo:02}")),
        ("YY", format!("{:02}", y.rem_euclid(100))),
        ("HH", format!("{hour:02}")),
        ("MM", format!("{mi:02}")),
        ("SS", format!("{:02}", sec as u32)),
        ("D", d.to_string()),
        ("M", mo.to_string()),
        ("H", hour.to_string()),
        ("Y", y.to_string()),
        ("S", (sec as u32).to_string()),
    ];
    let mut out = String::new();
    let mut rest = picture;
    'outer: while !rest.is_empty() {
        for (t, v) in &tokens {
            if rest.len() >= t.len() && rest.get(..t.len()).is_some_and(|p| p.eq_ignore_ascii_case(t)) {
                out.push_str(v);
                rest = rest.get(t.len()..).unwrap_or("");
                continue 'outer;
            }
        }
        let mut it = rest.chars();
        if let Some(ch) = it.next() {
            out.push(ch);
        }
        rest = it.as_str();
    }
    out
}

// ---------- RTEXT objects ----------

#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    File(String),
    Diesel(String),
}

fn source_tags(src: &Source) -> Vec<RawTag> {
    let (kind, text) = match src {
        Source::File(p) => (0, p),
        Source::Diesel(e) => (1, e),
    };
    let mut v = vec![RawTag { code: 1001, value: RTEXT_APP.into() }, RawTag { code: 1070, value: kind.to_string() }];
    let chars: Vec<char> = text.chars().collect();
    for part in chars.chunks(CHUNK).take(4096) {
        v.push(RawTag { code: 1000, value: part.iter().collect() });
    }
    v
}

/// An object's RTEXT source, from its xdata.
pub fn source_of(c: &Common) -> Option<Source> {
    let i = c.xdata.iter().position(|t| t.code == 1001 && t.value == RTEXT_APP)?;
    let rest = c.xdata.get(i + 1..)?;
    let kind = rest.first().filter(|t| t.code == 1070)?.value.trim().parse::<i32>().ok()?;
    let text: String = rest.iter().skip(1).take_while(|t| t.code == 1000).map(|t| t.value.as_str()).collect();
    Some(if kind == 1 { Source::Diesel(text) } else { Source::File(text) })
}

#[cfg(not(target_arch = "wasm32"))]
fn file_text(path: &str) -> String {
    use std::io::Read;
    let Ok(f) = std::fs::File::open(path) else { return format!("(file not found: {path})") };
    let mut bytes = Vec::new();
    if f.take(MAX_FILE).read_to_end(&mut bytes).is_err() {
        return format!("(file not readable: {path})");
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

#[cfg(target_arch = "wasm32")]
fn file_text(path: &str) -> String {
    format!("(files are not available in the web version: {path})")
}

/// What an RTEXT shows now, as MTEXT contents.
pub fn contents(s: &Session, src: &Source) -> String {
    let plain = match src {
        Source::File(p) => file_text(p),
        Source::Diesel(e) => diesel(s, e),
    };
    let t = plain.replace("\r\n", "\n").replace('\r', "\n");
    super::escape(t.trim_end_matches('\n')).replace('\n', "\\P")
}

/// Bring every RTEXT (or those among `only`) up to date: how many changed.
pub fn refresh(s: &mut Session, only: Option<&[Handle]>) -> Result<usize> {
    let d = s.doc()?;
    let mut found = Vec::new();
    for st in std::iter::once(&d.model).chain(d.layouts.iter().map(|l| &l.entities)) {
        for e in st.iter() {
            if !matches!(e.kind, EntityKind::MText(_)) || only.is_some_and(|o| !o.contains(&e.handle)) {
                continue;
            }
            if let Some(src) = source_of(&e.common) {
                found.push((e.handle, src));
            }
        }
    }
    let mut changed = 0;
    for (h, src) in found.into_iter().take(100_000) {
        let text = contents(s, &src);
        let same = matches!(s.doc()?.entity(h).map(|e| &e.kind), Some(EntityKind::MText(m)) if m.contents == text);
        if !same {
            s.doc_mut()?.modify_entity(h, |e| {
                if let EntityKind::MText(m) = &mut e.kind {
                    m.contents = text;
                }
            })?;
            changed += 1;
        }
    }
    Ok(changed)
}

fn run_rtext(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "rtext";
    let src = match (str_param(p, "file"), str_param(p, "diesel")) {
        (Some(f), None) => Source::File(f.to_string()),
        (None, Some(e)) => Source::Diesel(e.to_string()),
        _ => return Err(bad(cmd, "pass `file` (a text file) or `diesel` (an expression)")),
    };
    let at = p.get("at").and_then(point_value).ok_or_else(|| bad(cmd, "`at` (the start point) is required"))?;
    let d = s.doc()?;
    let height = p.get("height").and_then(Value::as_f64).filter(|h| h.is_finite() && *h > 0.0).unwrap_or_else(|| d.header.f64("TEXTSIZE", 2.5));
    let rotation = p.get("rotation").and_then(Value::as_f64).filter(|r| r.is_finite()).unwrap_or(0.0).to_radians();
    let style = str_param(p, "style").map_or_else(|| d.header.str("TEXTSTYLE", "Standard"), str::to_string);
    let m = MText {
        insert: Vec3::new(at.x, at.y, 0.0),
        height,
        width: 0.0,
        attach: 1,
        rotation,
        contents: contents(s, &src),
        style,
        line_spacing: 1.0,
        line_spacing_exact: false,
        columns: None,
        background: None,
    };
    let common = Common { xdata: source_tags(&src), ..s.current_common() };
    let space = s.space();
    let h = s.doc_mut()?.add(&space, common, EntityKind::MText(m))?;
    Ok(json!({ "handle": h.hex() }))
}

fn run_refresh(s: &mut Session, p: &Value) -> Result<Value> {
    let only = match p.get("handles") {
        Some(_) => Some(targets(s, p)?),
        None => None,
    };
    let n = refresh(s, only.as_deref())?;
    Ok(json!({ "changed": n }))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("rtext", "Remote Text", run_rtext)
            .menu(&["Tools", "Express Tools", "Text", "Remote Text"])
            .params("{file?: path | diesel?: expression, at: [x,y], height? (TEXTSIZE), rotation? (degrees), style? (TEXTSTYLE)} → {handle}: an MTEXT that REGEN brings up to date")
            .interactive(|s| Ok(Box::new(RtextM::new(s)))),
        CommandSpec::new("rtext.refresh", "Update Remote Text", run_refresh).params("{handles? (default every RTEXT)} → {changed}"),
    ]
}

#[derive(Clone, Copy, PartialEq)]
enum Ask {
    Option,
    Style,
    Height,
    Rotation,
    File,
    Diesel,
    Point,
}

struct RtextM {
    ask: Ask,
    style: String,
    height: f64,
    rotation: f64,
    src: Option<Source>,
}

impl RtextM {
    fn new(s: &Session) -> RtextM {
        let (style, height) =
            s.doc().map(|d| (d.header.str("TEXTSTYLE", "Standard"), d.header.f64("TEXTSIZE", 2.5))).unwrap_or(("Standard".into(), 2.5));
        RtextM { ask: Ask::Option, style, height, rotation: 0.0, src: None }
    }
}

impl Interactive for RtextM {
    fn name(&self) -> &'static str {
        "RTEXT"
    }
    fn begin(&mut self, s: &mut Session) -> Result<Step> {
        s.echo(format!("Current settings: Style={} Height={} Rotation={}", self.style, self.height, self.rotation));
        Ok(Step::Continue)
    }
    fn prompt(&self, _: &Session) -> Prompt {
        match self.ask {
            Ask::Option => Prompt::new("Enter an option", Accept::TEXT).kw(&["Style", "Height", "Rotation", "File", "Diesel"]).default("File"),
            Ask::Style => Prompt::new("Enter name of text style", Accept::TEXT).default(self.style.clone()),
            Ask::Height => Prompt::new("Specify height", Accept::TEXT).default(format!("{}", self.height)),
            Ask::Rotation => Prompt::new("Specify rotation angle", Accept::TEXT).default(format!("{}", self.rotation)),
            Ask::File => Prompt::new("Enter file name", Accept::TEXT),
            Ask::Diesel => Prompt::new("Enter DIESEL expression", Accept::TEXT),
            Ask::Point => Prompt::new("Specify start point of RText", Accept::POINT),
        }
    }
    fn input(&mut self, s: &mut Session, i: Input) -> Result<Step> {
        if matches!(i, Input::Cancel) {
            return Ok(Step::Cancel);
        }
        let t = match &i {
            Input::Text(t) | Input::Keyword(t) => t.trim().to_string(),
            _ => String::new(),
        };
        match self.ask {
            Ask::Option => {
                self.ask = match t.to_ascii_lowercase().chars().next() {
                    Some('s') => Ask::Style,
                    Some('h') => Ask::Height,
                    Some('r') => Ask::Rotation,
                    Some('d') => Ask::Diesel,
                    None | Some('f') => Ask::File,
                    _ => {
                        s.echo("Invalid option keyword.");
                        Ask::Option
                    }
                }
            }
            Ask::Style => {
                if !t.is_empty() {
                    self.style = t;
                }
                self.ask = Ask::Option;
            }
            Ask::Height | Ask::Rotation => {
                if !t.is_empty() {
                    match super::super::machines::number(&t) {
                        Some(v) if self.ask == Ask::Rotation && v.is_finite() => self.rotation = v,
                        Some(v) if v.is_finite() && v > 0.0 => self.height = v,
                        _ => {
                            s.echo("Requires a number.");
                            return Ok(Step::Continue);
                        }
                    }
                }
                self.ask = Ask::Option;
            }
            Ask::File | Ask::Diesel => {
                if t.is_empty() {
                    return Ok(Step::Cancel);
                }
                self.src = Some(if self.ask == Ask::File { Source::File(t.trim_matches('"').to_string()) } else { Source::Diesel(t) });
                self.ask = Ask::Point;
            }
            Ask::Point => {
                let Input::Point(p) = i else { return Ok(Step::Continue) };
                let mut params = json!({ "at": [p.x, p.y], "height": self.height, "rotation": self.rotation, "style": self.style });
                if let Some(o) = params.as_object_mut() {
                    match &self.src {
                        Some(Source::File(f)) => o.insert("file".into(), json!(f)),
                        Some(Source::Diesel(e)) => o.insert("diesel".into(), json!(e)),
                        None => return Ok(Step::Cancel),
                    };
                }
                if let Err(e) = run_rtext(s, &params) {
                    s.echo(e.to_string());
                }
                return Ok(Step::Done);
            }
        }
        Ok(Step::Continue)
    }
}
