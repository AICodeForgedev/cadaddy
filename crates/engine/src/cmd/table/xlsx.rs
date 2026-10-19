//! Excel workbooks (.xlsx, Office Open XML SpreadsheetML, ECMA-376): read a sheet's cells as
//! text the way Excel shows them, and write a table as a one-sheet workbook. An .xlsx file is a
//! ZIP archive of XML parts, read and written with the archive code ETRANSMIT uses and a small
//! XML reader here; no spreadsheet library.
//!
//! Read: the workbook's sheets and their relationships, shared strings (rich text runs joined),
//! inline strings, numbers, booleans, errors and formulas' cached values; numbers formatted by
//! their cell's number format (built-in formats 0–4, 9–22, and custom ones: decimals,
//! thousands separators, percent, dates and times). A range (`A1:C5`) picks part of the sheet.

use std::collections::HashMap;

use crate::cmd::file::zip;

/// Most cells read from one sheet.
const MAX_CELLS: usize = 1_000_000;
/// Largest row and column read (Excel's own limits).
const MAX_ROW: usize = 1_048_576;
const MAX_COL: usize = 16_384;

// ---------------- XML ----------------

/// One element: its name (without namespace prefix), attributes and children; text is kept in
/// order as `Node::Text`.
#[derive(Debug, Default)]
struct Element {
    name: String,
    attrs: Vec<(String, String)>,
    children: Vec<Node>,
}

#[derive(Debug)]
enum Node {
    El(Element),
    Text(String),
}

impl Element {
    fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == name || k.rsplit(':').next() == Some(name)).map(|(_, v)| v.as_str())
    }
    fn elements(&self) -> impl Iterator<Item = &Element> {
        self.children.iter().filter_map(|n| if let Node::El(e) = n { Some(e) } else { None })
    }
    fn child(&self, name: &str) -> Option<&Element> {
        self.elements().find(|e| e.name == name)
    }
    /// All text inside, in order.
    fn text(&self) -> String {
        let mut s = String::new();
        for n in &self.children {
            match n {
                Node::Text(t) => s.push_str(t),
                Node::El(e) => s.push_str(&e.text()),
            }
        }
        s
    }
    /// Descendants named `name`, depth first.
    fn find_all<'a>(&'a self, name: &str, out: &mut Vec<&'a Element>) {
        for e in self.elements() {
            if e.name == name {
                out.push(e);
            }
            e.find_all(name, out);
        }
    }
}

fn local(name: &str) -> String {
    name.rsplit(':').next().unwrap_or(name).to_string()
}

/// `&amp;`, `&#65;`, `&#x41;` … in text and attribute values.
fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(rest.get(..i).unwrap_or(""));
        let tail = rest.get(i..).unwrap_or("");
        let Some(end) = tail.find(';').filter(|e| *e <= 10) else {
            out.push('&');
            rest = tail.get(1..).unwrap_or("");
            continue;
        };
        let ent = tail.get(1..end).unwrap_or("");
        let c = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => ent
                .strip_prefix("#x")
                .or_else(|| ent.strip_prefix("#X"))
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| ent.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match c {
            Some(c) => out.push(c),
            None => out.push_str(tail.get(..=end).unwrap_or("")),
        }
        rest = tail.get(end + 1..).unwrap_or("");
    }
    out.push_str(rest);
    out
}

/// Parse an XML document into its root element (namespaces dropped from names). Malformed input
/// gives what could be read.
fn parse_xml(src: &str) -> Option<Element> {
    let mut stack: Vec<Element> = vec![Element::default()];
    let mut rest = src;
    while !rest.is_empty() {
        let Some(lt) = rest.find('<') else {
            push_text(&mut stack, rest);
            break;
        };
        push_text(&mut stack, rest.get(..lt).unwrap_or(""));
        rest = rest.get(lt..).unwrap_or("");
        if let Some(r) = rest.strip_prefix("<!--") {
            rest = r.find("-->").and_then(|e| r.get(e + 3..)).unwrap_or("");
            continue;
        }
        if let Some(r) = rest.strip_prefix("<![CDATA[") {
            let end = r.find("]]>").unwrap_or(r.len());
            if let Some(top) = stack.last_mut() {
                top.children.push(Node::Text(r.get(..end).unwrap_or("").to_string()));
            }
            rest = r.get(end + 3..).unwrap_or("");
            continue;
        }
        if rest.starts_with("<?") || rest.starts_with("<!") {
            rest = rest.find('>').and_then(|e| rest.get(e + 1..)).unwrap_or("");
            continue;
        }
        let gt = find_tag_end(rest)?;
        let tag = rest.get(1..gt).unwrap_or("");
        rest = rest.get(gt + 1..).unwrap_or("");
        if let Some(name) = tag.strip_prefix('/') {
            let name = local(name.trim());
            // Close up to the matching element.
            while stack.len() > 1 {
                let Some(done) = stack.pop() else { break };
                let matched = done.name == name;
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(Node::El(done));
                }
                if matched {
                    break;
                }
            }
            continue;
        }
        let selfclose = tag.ends_with('/');
        let tag = tag.trim_end_matches('/');
        let (name, attrs) = parse_tag(tag);
        let el = Element { name, attrs, children: Vec::new() };
        if selfclose {
            if let Some(top) = stack.last_mut() {
                top.children.push(Node::El(el));
            }
        } else {
            if stack.len() > 256 {
                return None;
            }
            stack.push(el);
        }
    }
    while stack.len() > 1 {
        let done = stack.pop()?;
        stack.last_mut()?.children.push(Node::El(done));
    }
    let root = stack.pop()?;
    root.children.into_iter().find_map(|n| if let Node::El(e) = n { Some(e) } else { None })
}

fn push_text(stack: &mut [Element], t: &str) {
    if t.trim().is_empty() && !t.contains('\u{a0}') {
        // Whitespace between elements; text in <t> keeps its spaces (it is never only spaces
        // in the parts read here except xml:space="preserve" strings, kept below).
        if let Some(top) = stack.last_mut()
            && top.name == "t"
            && !t.is_empty()
        {
            top.children.push(Node::Text(t.to_string()));
        }
        return;
    }
    if let Some(top) = stack.last_mut() {
        top.children.push(Node::Text(unescape(t)));
    }
}

/// Where a tag ends (its `>` outside quoted attribute values).
fn find_tag_end(s: &str) -> Option<usize> {
    let mut quote: Option<char> = None;
    for (i, c) in s.char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            (None, '>') => return Some(i),
            _ => {}
        }
    }
    None
}

fn parse_tag(tag: &str) -> (String, Vec<(String, String)>) {
    let tag = tag.trim();
    let name_end = tag.find(|c: char| c.is_whitespace()).unwrap_or(tag.len());
    let name = local(tag.get(..name_end).unwrap_or(""));
    let mut attrs = Vec::new();
    let mut rest = tag.get(name_end..).unwrap_or("").trim_start();
    while let Some(eq) = rest.find('=') {
        let key = rest.get(..eq).unwrap_or("").trim().to_string();
        let after = rest.get(eq + 1..).unwrap_or("").trim_start();
        let Some(q) = after.chars().next().filter(|c| *c == '"' || *c == '\'') else { break };
        let body = after.get(1..).unwrap_or("");
        let end = body.find(q).unwrap_or(body.len());
        attrs.push((key, unescape(body.get(..end).unwrap_or(""))));
        rest = body.get(end + 1..).unwrap_or("").trim_start();
    }
    (name, attrs)
}

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c if (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r') => {}
            c => out.push(c),
        }
    }
    out
}

// ---------------- cell references ----------------

/// `"BC12"` → (row 11, column 54), 0-based.
pub fn cell_ref(r: &str) -> Option<(usize, usize)> {
    let r = r.trim().replace('$', "");
    let split = r.find(|c: char| c.is_ascii_digit())?;
    let (letters, digits) = r.split_at(split);
    if letters.is_empty() || letters.len() > 3 || !letters.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let col = letters.to_ascii_uppercase().bytes().try_fold(0usize, |acc, b| acc.checked_mul(26)?.checked_add(usize::from(b - b'A' + 1)))?;
    let row: usize = digits.parse().ok()?;
    ((1..=MAX_ROW).contains(&row) && (1..=MAX_COL).contains(&col)).then(|| (row - 1, col - 1))
}

fn col_name(mut c: usize) -> String {
    let mut s = Vec::new();
    c += 1;
    while c > 0 {
        let r = (c - 1) % 26;
        s.push(b'A' + r as u8);
        c = (c - 1) / 26;
    }
    s.reverse();
    String::from_utf8(s).unwrap_or_default()
}

// ---------------- number formats ----------------

/// The format code of built-in number format `id` (ECMA-376 Part 1, 18.8.30).
fn builtin_format(id: u32) -> Option<&'static str> {
    Some(match id {
        0 => "General",
        1 => "0",
        2 => "0.00",
        3 => "#,##0",
        4 => "#,##0.00",
        9 => "0%",
        10 => "0.00%",
        11 => "0.00E+00",
        12 => "# ?/?",
        13 => "# ??/??",
        14 => "mm-dd-yy",
        15 => "d-mmm-yy",
        16 => "d-mmm",
        17 => "mmm-yy",
        18 => "h:mm AM/PM",
        19 => "h:mm:ss AM/PM",
        20 => "h:mm",
        21 => "h:mm:ss",
        22 => "m/d/yy h:mm",
        49 => "@",
        _ => return None,
    })
}

/// A serial day number (1900 date system) as (year, month, day). Excel counts 29 Feb 1900,
/// which didn't exist: from serial 61 on, day 0 is 30 Dec 1899; before it, 31 Dec 1899.
fn serial_date(serial: f64) -> (i64, u32, u32) {
    let n = serial.floor() as i64;
    // Days since 1970-01-01.
    let days = if n >= 61 { n - 25_569 } else { n - 25_568 };
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// `v` as format `code` shows it (the parts used in drawings' tables; anything else: General).
pub fn format_number(v: f64, code: &str) -> String {
    let general = |v: f64| {
        if v.fract() == 0.0 && v.abs() < 1e15 {
            format!("{}", v as i64)
        } else {
            let s = format!("{:.10}", v);
            s.trim_end_matches('0').trim_end_matches('.').to_string()
        }
    };
    // The positive section only.
    let code = code.split(';').next().unwrap_or("General");
    // Strip quoted text, colours and escapes into the literal parts kept around the number.
    let lower = code.to_ascii_lowercase();
    if lower == "general" || code.is_empty() || code == "@" {
        return general(v);
    }
    let unquoted: String = {
        let mut out = String::new();
        let mut inq = false;
        for c in code.chars() {
            if c == '"' {
                inq = !inq;
            } else if !inq {
                out.push(c);
            }
        }
        out
    };
    let ul = unquoted.to_ascii_lowercase();
    let is_date =
        ul.contains('y') || ul.contains('d') || (ul.contains('m') && !ul.contains('0') && !ul.contains('#')) || ul.contains('h') || ul.contains("ss");
    if is_date && !ul.contains('0') {
        let (y, m, d) = serial_date(v);
        let secs = ((v.fract()) * 86_400.0).round() as i64;
        let (hh, mm, ss) = (secs / 3600, secs % 3600 / 60, secs % 60);
        let mut out = String::new();
        let chars: Vec<char> = unquoted.chars().collect();
        let mut i = 0;
        let has_time = ul.contains('h');
        while i < chars.len() {
            let c = chars.get(i).copied().unwrap_or(' ');
            let run = chars.iter().skip(i).take_while(|x| x.eq_ignore_ascii_case(&c)).count();
            match c.to_ascii_lowercase() {
                'y' => out.push_str(&if run <= 2 { format!("{:02}", y.rem_euclid(100)) } else { format!("{y:04}") }),
                // `m` after `h` or before `s` is minutes.
                'm' if has_time && (out.ends_with(':') || chars.get(i + run) == Some(&':')) => out.push_str(&format!("{mm:02}")),
                'm' => out.push_str(&match run {
                    1 => m.to_string(),
                    2 => format!("{m:02}"),
                    3 => MONTHS.get(m as usize - 1).copied().unwrap_or("").to_string(),
                    _ => MONTHS.get(m as usize - 1).copied().unwrap_or("").to_string(),
                }),
                'd' => out.push_str(&if run == 1 { d.to_string() } else { format!("{d:02}") }),
                'h' => out.push_str(&if ul.contains("am/pm") {
                    (((hh + 11) % 12) + 1).to_string()
                } else if run == 1 {
                    hh.to_string()
                } else {
                    format!("{hh:02}")
                }),
                's' => out.push_str(&format!("{ss:02}")),
                'a' if unquoted.get(i..i + 5).is_some_and(|s| s.eq_ignore_ascii_case("am/pm")) => {
                    out.push_str(if hh < 12 { "AM" } else { "PM" });
                    i += 5;
                    continue;
                }
                '\\' => {
                    if let Some(n) = chars.get(i + 1) {
                        out.push(*n);
                    }
                    i += 2;
                    continue;
                }
                _ => out.extend(std::iter::repeat_n(c, run)),
            }
            i += run;
        }
        return out;
    }
    let percent = unquoted.contains('%');
    let v = if percent { v * 100.0 } else { v };
    let num_part: String = unquoted.chars().filter(|c| matches!(c, '0' | '#' | ',' | '.' | '?')).collect();
    if num_part.is_empty() {
        return general(v);
    }
    let decimals = num_part.split_once('.').map_or(0, |(_, f)| f.chars().filter(|c| matches!(c, '0' | '#' | '?')).count());
    let thousands = num_part.split('.').next().unwrap_or("").contains(',');
    let mut s = format!("{:.*}", decimals, v.abs());
    if thousands {
        let (int, frac) = s.split_once('.').map_or((s.clone(), None), |(a, b)| (a.to_string(), Some(b.to_string())));
        let mut grouped = String::new();
        for (k, ch) in int.chars().enumerate() {
            if k > 0 && (int.len() - k) % 3 == 0 {
                grouped.push(',');
            }
            grouped.push(ch);
        }
        s = match frac {
            Some(f) => format!("{grouped}.{f}"),
            None => grouped,
        };
    }
    // Literal text before and after the number part (currency signs, units), quoted or not.
    let mut first = None;
    let mut last = 0;
    let mut inq = false;
    for (i, c) in code.char_indices() {
        if c == '"' {
            inq = !inq;
        } else if !inq && matches!(c, '0' | '#' | '?' | ',' | '.') {
            first.get_or_insert(i);
            last = i + 1;
        }
    }
    let first = first.unwrap_or(0);
    let clean = |t: &str| t.replace(['"', '\\', '_', '*', '%'], "");
    let before = clean(code.get(..first).unwrap_or(""));
    let after = clean(code.get(last..).unwrap_or(""));
    let sign = if v < 0.0 && s.chars().any(|c| c.is_ascii_digit() && c != '0') { "-" } else { "" };
    format!("{sign}{before}{s}{after}{}", if percent { "%" } else { "" })
}

// ---------------- workbook ----------------

/// A workbook read from an .xlsx file: its sheet names, and per sheet its cells as shown.
pub struct Workbook {
    parts: HashMap<String, Vec<u8>>,
    pub sheets: Vec<(String, String)>,
    shared: Vec<String>,
    /// Per cell style index, its number format code.
    formats: Vec<String>,
}

fn part_text(parts: &HashMap<String, Vec<u8>>, name: &str) -> Option<String> {
    parts.get(name).map(|b| String::from_utf8_lossy(b).into_owned())
}

/// Read an .xlsx file's bytes.
pub fn open(bytes: &[u8]) -> Result<Workbook, String> {
    let entries = zip::read(bytes).map_err(|e| format!("not an .xlsx workbook: {e}"))?;
    let parts: HashMap<String, Vec<u8>> = entries.into_iter().map(|(n, b)| (n.trim_start_matches('/').to_string(), b)).collect();
    let wb = part_text(&parts, "xl/workbook.xml").and_then(|t| parse_xml(&t)).ok_or("not an .xlsx workbook (no xl/workbook.xml)")?;
    let rels: HashMap<String, String> = part_text(&parts, "xl/_rels/workbook.xml.rels")
        .and_then(|t| parse_xml(&t))
        .map(|r| {
            r.elements()
                .filter_map(|e| {
                    let target = e.attr("Target")?;
                    let path = if let Some(abs) = target.strip_prefix('/') { abs.to_string() } else { format!("xl/{target}") };
                    Some((e.attr("Id")?.to_string(), path))
                })
                .collect()
        })
        .unwrap_or_default();
    let mut sheet_els = Vec::new();
    wb.find_all("sheet", &mut sheet_els);
    let sheets: Vec<(String, String)> = sheet_els
        .iter()
        .enumerate()
        .filter_map(|(i, s)| {
            let name = s.attr("name")?.to_string();
            let path = s.attr("id").and_then(|id| rels.get(id).cloned()).unwrap_or_else(|| format!("xl/worksheets/sheet{}.xml", i + 1));
            Some((name, path))
        })
        .collect();
    let shared: Vec<String> = part_text(&parts, "xl/sharedStrings.xml")
        .and_then(|t| parse_xml(&t))
        .map(|sst| {
            sst.elements()
                .filter(|e| e.name == "si")
                .map(|si| {
                    // Plain text, or rich runs (`r`) whose `t`s join; phonetic runs (`rPh`) are left out.
                    let mut s = String::new();
                    for e in si.elements() {
                        match e.name.as_str() {
                            "t" => s.push_str(&e.text()),
                            "r" => s.push_str(&e.child("t").map(Element::text).unwrap_or_default()),
                            _ => {}
                        }
                    }
                    s
                })
                .collect()
        })
        .unwrap_or_default();
    let formats = part_text(&parts, "xl/styles.xml")
        .and_then(|t| parse_xml(&t))
        .map(|st| {
            let custom: HashMap<u32, String> = st
                .child("numFmts")
                .map(|n| n.elements().filter_map(|f| Some((f.attr("numFmtId")?.parse().ok()?, f.attr("formatCode")?.to_string()))).collect())
                .unwrap_or_default();
            st.child("cellXfs")
                .map(|x| {
                    x.elements()
                        .map(|xf| {
                            let id: u32 = xf.attr("numFmtId").and_then(|v| v.parse().ok()).unwrap_or(0);
                            custom.get(&id).cloned().or_else(|| builtin_format(id).map(str::to_string)).unwrap_or_else(|| "General".into())
                        })
                        .collect()
                })
                .unwrap_or_default()
        })
        .unwrap_or_default();
    Ok(Workbook { parts, sheets, shared, formats })
}

impl Workbook {
    /// The cells of sheet `name` (default the first), limited to `range` (`A1:C5`), as rows of
    /// text, empty cells as empty strings.
    pub fn cells(&self, sheet: Option<&str>, range: Option<&str>) -> Result<Vec<Vec<String>>, String> {
        let (name, path) = match sheet {
            Some(n) => self.sheets.iter().find(|(s, _)| s.eq_ignore_ascii_case(n.trim())).ok_or_else(|| format!("no sheet \"{n}\""))?,
            None => self.sheets.first().ok_or("the workbook has no sheets")?,
        };
        let xml = part_text(&self.parts, path).and_then(|t| parse_xml(&t)).ok_or_else(|| format!("sheet \"{name}\" can't be read"))?;
        let bounds = match range {
            Some(r) => {
                let (a, b) = r.split_once(':').unwrap_or((r, r));
                let (a, b) = (cell_ref(a).ok_or_else(|| format!("bad range {r}"))?, cell_ref(b).ok_or_else(|| format!("bad range {r}"))?);
                Some(((a.0.min(b.0), a.1.min(b.1)), (a.0.max(b.0), a.1.max(b.1))))
            }
            None => None,
        };
        let mut cells: Vec<(usize, usize, String)> = Vec::new();
        let mut rows = Vec::new();
        xml.find_all("row", &mut rows);
        for (ri, row) in rows.iter().enumerate() {
            let rnum = row.attr("r").and_then(|v| v.parse::<usize>().ok()).map_or(ri, |n| n.saturating_sub(1));
            for (ci, c) in row.elements().filter(|e| e.name == "c").enumerate() {
                let (r, col) = c.attr("r").and_then(cell_ref).unwrap_or((rnum, ci));
                if let Some(((r0, c0), (r1, c1))) = bounds
                    && (r < r0 || r > r1 || col < c0 || col > c1)
                {
                    continue;
                }
                let v = c.child("v").map(Element::text);
                let text = match c.attr("t").unwrap_or("n") {
                    "s" => v.and_then(|i| i.trim().parse::<usize>().ok()).and_then(|i| self.shared.get(i).cloned()).unwrap_or_default(),
                    "inlineStr" => c
                        .child("is")
                        .map(|is| {
                            is.elements()
                                .map(|e| if e.name == "r" { e.child("t").map(Element::text).unwrap_or_default() } else { e.text() })
                                .collect()
                        })
                        .unwrap_or_default(),
                    "str" | "e" => v.unwrap_or_default(),
                    "b" => {
                        if v.as_deref().map(str::trim) == Some("1") {
                            "TRUE".into()
                        } else {
                            "FALSE".into()
                        }
                    }
                    _ => match v.and_then(|s| s.trim().parse::<f64>().ok()).filter(|n| n.is_finite()) {
                        Some(n) => {
                            let style = c.attr("s").and_then(|s| s.parse::<usize>().ok()).unwrap_or(0);
                            format_number(n, self.formats.get(style).map_or("General", String::as_str))
                        }
                        None => String::new(),
                    },
                };
                cells.push((r, col, text));
                if cells.len() > MAX_CELLS {
                    return Err(format!("sheet \"{name}\" has more than {MAX_CELLS} cells"));
                }
            }
        }
        let (r0, c0) = bounds.map_or_else(|| (cells.iter().map(|c| c.0).min().unwrap_or(0), cells.iter().map(|c| c.1).min().unwrap_or(0)), |b| b.0);
        let (r1, c1) = bounds.map_or_else(|| (cells.iter().map(|c| c.0).max().unwrap_or(0), cells.iter().map(|c| c.1).max().unwrap_or(0)), |b| b.1);
        if cells.is_empty() && bounds.is_none() {
            return Ok(Vec::new());
        }
        let (h, w) = (r1.saturating_sub(r0) + 1, c1.saturating_sub(c0) + 1);
        if h.saturating_mul(w) > MAX_CELLS {
            return Err("the range is too large".into());
        }
        let mut grid = vec![vec![String::new(); w]; h];
        for (r, c, t) in cells {
            if let Some(cell) = grid.get_mut(r.wrapping_sub(r0)).and_then(|row| row.get_mut(c.wrapping_sub(c0))) {
                *cell = t;
            }
        }
        // Trailing empty rows go.
        while grid.last().is_some_and(|r| r.iter().all(String::is_empty)) && bounds.is_none() {
            grid.pop();
        }
        Ok(grid)
    }
}

/// A one-sheet workbook holding `rows` (numbers as numbers, the rest as text).
pub fn write(rows: &[Vec<String>], sheet: &str) -> Result<Vec<u8>, String> {
    let sheet = xml_escape(if sheet.trim().is_empty() { "Sheet1" } else { sheet.trim() });
    let mut data = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>"#,
    );
    for (r, row) in rows.iter().enumerate() {
        data.push_str(&format!(r#"<row r="{}">"#, r + 1));
        for (c, v) in row.iter().enumerate() {
            if v.is_empty() {
                continue;
            }
            let at = format!("{}{}", col_name(c), r + 1);
            // A number stays a number when Excel would show it the same way; text that only looks
            // like one ("99.00", leading zeros, spaces, +) stays text, as the table shows it.
            let numeric = v.parse::<f64>().is_ok_and(|n| n.is_finite() && format_number(n, "General") == *v);
            if numeric {
                data.push_str(&format!(r#"<c r="{at}"><v>{v}</v></c>"#));
            } else {
                data.push_str(&format!(r#"<c r="{at}" t="inlineStr"><is><t xml:space="preserve">{}</t></is></c>"#, xml_escape(v)));
            }
        }
        data.push_str("</row>");
    }
    data.push_str("</sheetData></worksheet>");
    let parts: Vec<(String, Vec<u8>)> = vec![
        ("[Content_Types].xml".into(), r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>"#.into()),
        ("_rels/.rels".into(), r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#.into()),
        ("xl/workbook.xml".into(), format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="{sheet}" sheetId="1" r:id="rId1"/></sheets></workbook>"#).into_bytes()),
        ("xl/_rels/workbook.xml.rels".into(), r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#.into()),
        ("xl/worksheets/sheet1.xml".into(), data.into_bytes()),
    ];
    zip::write(&parts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_and_references() {
        let e = parse_xml(r#"<?xml version="1.0"?><a x:k="1 &amp; 2"><!-- c --><b>t&lt;x&gt;</b><c/><![CDATA[<raw>]]></a>"#).unwrap();
        assert_eq!(e.name, "a");
        assert_eq!(e.attr("k"), Some("1 & 2"));
        assert_eq!(e.child("b").unwrap().text(), "t<x>");
        assert!(e.text().contains("<raw>"));
        assert_eq!(cell_ref("A1"), Some((0, 0)));
        assert_eq!(cell_ref("$AB$12"), Some((11, 27)));
        assert_eq!(cell_ref("1A"), None);
        assert_eq!(col_name(27), "AB");
        assert!(parse_xml("<a><b></a>").is_some(), "unbalanced input still reads");
    }

    #[test]
    fn number_formats() {
        assert_eq!(format_number(1234.5, "General"), "1234.5");
        assert_eq!(format_number(1234.5, "#,##0.00"), "1,234.50");
        assert_eq!(format_number(0.256, "0.0%"), "25.6%");
        assert_eq!(format_number(3.0, "0"), "3");
        assert_eq!(format_number(45_000.0, "yyyy-mm-dd"), "2023-03-15");
        assert_eq!(format_number(45_000.0, "mm-dd-yy"), "03-15-23");
        assert_eq!(format_number(45_000.0, "d-mmm-yy"), "15-Mar-23");
        assert_eq!(format_number(0.5, "h:mm"), "12:00");
        assert_eq!(format_number(12.5, "\"$\"#,##0.00"), "$12.50");
        assert_eq!(format_number(-2.0, "0.00"), "-2.00");
    }
}
