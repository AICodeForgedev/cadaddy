//! Readers and writers for the user's own linetype (`.lin`) and hatch pattern (`.pat`) files.
//!
//! Both follow the publicly documented text formats: a header line `*NAME[,description]`, then
//! the definition (`A,dash,gap,…` for a linetype, `angle, x, y, dx, dy[, dash, …]` lines for
//! a pattern), with `;` starting a comment line. CADaddy ships no such file; these read the
//! files users bring. Everything here treats the text as hostile: lines, definitions, dashes,
//! families and warnings are capped, numbers must be finite and reasonable, and nothing panics.
//! Problems become warnings and the definitions that can be read are kept.

use super::PatternLine;
use crate::{DashElement, Linetype};

/// Longest line read; longer lines are skipped with a warning.
pub const MAX_LINE_CHARS: usize = 4096;
/// Most definitions (linetypes or patterns) read from one file.
pub const MAX_DEFINITIONS: usize = 4096;
/// Most elements in one linetype.
pub const MAX_LINETYPE_ELEMENTS: usize = 256;
/// Most line families in one hatch pattern (what the renderer draws, too).
pub const MAX_PATTERN_FAMILIES: usize = 1024;
/// Most dashes in one pattern line family (as for a pattern definition read from DXF).
pub const MAX_PATTERN_DASHES: usize = 256;
/// Largest magnitude accepted for a length, offset or scale.
pub const MAX_VALUE: f64 = 1.0e6;
/// Longest name or description kept.
const MAX_NAME_CHARS: usize = 255;
/// Most warnings collected; the rest are counted in a last line.
const MAX_WARNINGS: usize = 100;

/// A hatch pattern read from a user's `.pat` file.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct UserPattern {
    pub name: String,
    pub description: String,
    pub lines: Vec<PatternLine>,
}

/// What a file held: the definitions that could be read, and why anything else was skipped.
#[derive(Clone, Debug, PartialEq)]
pub struct Parsed<T> {
    pub items: Vec<T>,
    pub warnings: Vec<String>,
}

impl<T> Default for Parsed<T> {
    fn default() -> Self {
        Parsed { items: Vec::new(), warnings: Vec::new() }
    }
}

impl<T> Parsed<T> {
    fn warn(&mut self, line: usize, msg: impl Into<String>) {
        match self.warnings.len() {
            n if n < MAX_WARNINGS => self.warnings.push(format!("line {line}: {}", msg.into())),
            MAX_WARNINGS => self.warnings.push("more problems not listed".into()),
            _ => {}
        }
    }
}

/// A header line `*NAME[,description]`: the name and description, or why it can't be used.
fn header(rest: &str) -> std::result::Result<(String, String), String> {
    let (name, desc) = rest.split_once(',').unwrap_or((rest, ""));
    let name = name.trim();
    if name.is_empty() {
        return Err("a definition without a name".into());
    }
    if name.chars().count() > MAX_NAME_CHARS || name.chars().any(char::is_control) {
        return Err("a definition name that is too long or has control characters".into());
    }
    let desc: String = desc.trim().chars().filter(|c| !c.is_control()).take(MAX_NAME_CHARS).collect();
    Ok((name.to_string(), desc))
}

/// A finite number within ±[`MAX_VALUE`].
fn number(t: &str) -> Option<f64> {
    t.trim().parse::<f64>().ok().filter(|v| v.is_finite() && v.abs() <= MAX_VALUE)
}

/// The file's text lines with 1-based numbers, without a leading byte-order mark.
fn lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.trim_start_matches('\u{feff}').lines().enumerate().map(|(i, l)| (i.saturating_add(1), l.trim()))
}

// ---------- LIN ----------

/// Read the linetypes of a `.lin` file. Simple linetypes and complex ones with embedded text
/// (`["TEXT",STYLE,S=…,R=…,A=…,U=…,X=…,Y=…]`) are read; shape elements (`[SHAPE,file.shx,…]`)
/// need SHX shape fonts, so they are left out with a warning and the dashes around them kept.
/// Lengths are in drawing units as written: a user's file is never rescaled.
pub fn parse_lin(text: &str) -> Parsed<Linetype> {
    let mut out = Parsed::default();
    // The definition being read: name, description, header line.
    let mut pending: Option<(String, String, usize)> = None;
    for (no, line) in lines(text) {
        if line.is_empty() || line.starts_with(';') {
            continue;
        }
        if line.chars().count() > MAX_LINE_CHARS {
            out.warn(no, "line too long; skipped");
            continue;
        }
        if let Some(rest) = line.strip_prefix('*') {
            if let Some((name, _, at)) = pending.take() {
                out.warn(at, format!("linetype {name} has no pattern line; skipped"));
            }
            match header(rest) {
                Ok((name, desc)) => pending = Some((name, desc, no)),
                Err(e) => out.warn(no, format!("{e}; skipped")),
            }
            continue;
        }
        let Some((name, description, _)) = pending.take() else {
            out.warn(no, "pattern line without a *NAME line before it; skipped");
            continue;
        };
        if out.items.len() >= MAX_DEFINITIONS {
            out.warn(no, format!("more than {MAX_DEFINITIONS} linetypes; the rest are skipped"));
            break;
        }
        if out.items.iter().any(|l: &Linetype| l.name.eq_ignore_ascii_case(&name)) {
            out.warn(no, format!("linetype {name} is defined twice; the first definition is kept"));
            continue;
        }
        match lin_pattern(line) {
            Ok(pattern) => out.items.push(Linetype { name, description, pattern }),
            Err(e) => out.warn(no, format!("linetype {name}: {e}; skipped")),
        }
    }
    if let Some((name, _, at)) = pending {
        out.warn(at, format!("linetype {name} has no pattern line; skipped"));
    }
    out
}

/// Split a pattern line at commas outside `[…]` and `"…"`.
fn lin_fields(line: &str) -> std::result::Result<Vec<&str>, String> {
    let mut out = Vec::new();
    let (mut depth, mut quoted, mut start) = (0usize, false, 0usize);
    for (i, c) in line.char_indices() {
        match c {
            '"' => quoted = !quoted,
            '[' if !quoted => depth = depth.saturating_add(1),
            ']' if !quoted => depth = depth.checked_sub(1).ok_or("a ] without its [")?,
            ',' if !quoted && depth == 0 => {
                out.push(line.get(start..i).unwrap_or("").trim());
                start = i.saturating_add(1);
            }
            _ => {}
        }
        if out.len() > MAX_LINETYPE_ELEMENTS.saturating_mul(2) {
            return Err(format!("more than {MAX_LINETYPE_ELEMENTS} elements"));
        }
    }
    if quoted || depth != 0 {
        return Err("an unclosed [ or \"".into());
    }
    out.push(line.get(start..).unwrap_or("").trim());
    Ok(out)
}

/// The elements of a pattern line `A,dash,gap,[…],…`.
fn lin_pattern(line: &str) -> std::result::Result<Vec<DashElement>, String> {
    let mut fields = lin_fields(line)?;
    // The alignment field ("A") comes first; a line of plain numbers is read as well.
    if fields.first().is_some_and(|f| f.eq_ignore_ascii_case("a")) {
        fields.remove(0);
    } else if fields.first().is_some_and(|f| number(f).is_none() && !f.starts_with('[')) {
        return Err(format!("unknown alignment `{}` (expected A)", fields.first().copied().unwrap_or("")));
    }
    let mut pattern: Vec<DashElement> = Vec::new();
    for f in fields.into_iter().filter(|f| !f.is_empty()) {
        if pattern.len() >= MAX_LINETYPE_ELEMENTS {
            return Err(format!("more than {MAX_LINETYPE_ELEMENTS} elements"));
        }
        let Some(inner) = f.strip_prefix('[') else {
            let len = number(f).ok_or_else(|| format!("`{f}` is not a dash length"))?;
            pattern.push(DashElement::dash(len));
            continue;
        };
        let inner = inner.strip_suffix(']').ok_or_else(|| format!("`{f}` is not a complete [ ] element"))?;
        let el = match lin_text(inner)? {
            Some(text) => text,
            None => lin_shape(inner)?,
        };
        // A text or shape sits at the end of the element before it (a zero-length one if none).
        if pattern.last().is_none_or(|e| e.text.is_some() || e.shape_name.is_some() || e.shape.is_some()) {
            pattern.push(DashElement::dash(0.0));
        }
        if let Some(last) = pattern.last_mut() {
            *last = DashElement { length: last.length, ..el };
        }
    }
    if pattern.is_empty() {
        return Err("no dashes".into());
    }
    if pattern.iter().all(|e| e.length == 0.0) && pattern.iter().all(|e| e.text.is_none() && e.shape_name.is_none()) {
        return Err("every element has zero length".into());
    }
    Ok(pattern)
}

/// A `SHAPE,FILE.shx,key=value…` element (the inside of `[…]`): the shape by name; its number is
/// looked up in the shape file when the linetype is loaded.
fn lin_shape(inner: &str) -> std::result::Result<DashElement, String> {
    let mut parts = inner.split(',').map(str::trim);
    let shape = parts.next().unwrap_or("");
    if shape.is_empty() || shape.chars().count() > 64 || !shape.chars().all(|c| c.is_alphanumeric() || "_-$".contains(c)) {
        return Err(format!("shape element: bad shape name `{shape}`"));
    }
    let file = parts.next().unwrap_or("").trim();
    if file.is_empty() || file.contains('=') || file.chars().count() > MAX_NAME_CHARS || file.chars().any(char::is_control) {
        return Err(format!("shape element {shape}: the shape file is missing"));
    }
    let mut e = DashElement { shape_name: Some(shape.to_ascii_uppercase()), style: Some(file.to_string()), ..DashElement::dash(0.0) };
    options(&mut e, parts, "shape element")?;
    Ok(e)
}

/// The `S=`, `X=`, `Y=`, `R=`/`A=`/`U=` options of a text or shape element.
fn options<'a>(e: &mut DashElement, parts: impl Iterator<Item = &'a str>, what: &str) -> std::result::Result<(), String> {
    for kv in parts.filter(|p| !p.is_empty()) {
        let (k, v) = kv.split_once('=').ok_or_else(|| format!("{what}: `{kv}` is not key=value"))?;
        match k.trim().to_ascii_uppercase().as_str() {
            "S" => e.scale = number(v).filter(|s| *s > 0.0).ok_or_else(|| format!("{what}: bad scale `{v}`"))?,
            "X" => e.offset.x = number(v).ok_or_else(|| format!("{what}: bad X offset `{v}`"))?,
            "Y" => e.offset.y = number(v).ok_or_else(|| format!("{what}: bad Y offset `{v}`"))?,
            // Upright text keeps a rotation relative to the line, like R.
            k @ ("R" | "A" | "U") => {
                e.rotation = angle(v).ok_or_else(|| format!("{what}: bad rotation `{v}`"))?;
                e.absolute = k == "A";
            }
            other => return Err(format!("{what}: unknown option `{other}`")),
        }
    }
    Ok(())
}

/// A `"TEXT",STYLE,key=value…` element (the inside of `[…]`), or `None` for a shape element.
fn lin_text(inner: &str) -> std::result::Result<Option<DashElement>, String> {
    let inner = inner.trim();
    let Some(after) = inner.strip_prefix('"') else { return Ok(None) };
    let (text, rest) = after.split_once('"').ok_or("unclosed text quote")?;
    if text.chars().count() > MAX_NAME_CHARS || text.chars().any(char::is_control) {
        return Err("embedded text that is too long or has control characters".into());
    }
    let mut parts = rest.split(',').map(str::trim);
    if !parts.next().unwrap_or("").is_empty() {
        return Err("text element: expected a comma after the text".into());
    }
    let style = parts.next().unwrap_or("").trim();
    if style.is_empty() || style.contains('=') {
        return Err("text element: the text style is missing".into());
    }
    let mut e = DashElement { text: Some(text.to_string()), style: Some(style.to_string()), ..DashElement::dash(0.0) };
    options(&mut e, parts, "text element")?;
    Ok(Some(e))
}

/// An angle in degrees, or with a `d` (degrees), `r` (radians) or `g` (grads) suffix; radians out.
fn angle(t: &str) -> Option<f64> {
    let t = t.trim().to_ascii_lowercase();
    let (num, unit) = match t.char_indices().last() {
        Some((i, c @ ('d' | 'r' | 'g'))) => (t.get(..i)?, c),
        _ => (t.as_str(), 'd'),
    };
    let v = num.trim().parse::<f64>().ok().filter(|v| v.is_finite() && v.abs() <= 1.0e5)?;
    Some(match unit {
        'r' => v,
        'g' => v * std::f64::consts::PI / 200.0,
        _ => v.to_radians(),
    })
}

// ---------- PAT ----------

/// Read the hatch patterns of a `.pat` file. Values are used as written (an inch file for inch
/// drawings, a millimetre file for metric ones).
pub fn parse_pat(text: &str) -> Parsed<UserPattern> {
    let mut out: Parsed<UserPattern> = Parsed::default();
    // The pattern being read, its header line, and whether it already failed.
    let mut cur: Option<(UserPattern, usize, bool)> = None;
    let finish = |cur: Option<(UserPattern, usize, bool)>, out: &mut Parsed<UserPattern>| {
        let Some((p, at, failed)) = cur else { return };
        if failed {
            return;
        }
        if p.lines.is_empty() {
            out.warn(at, format!("pattern {} has no line families; skipped", p.name));
        } else if out.items.len() >= MAX_DEFINITIONS {
            out.warn(at, format!("more than {MAX_DEFINITIONS} patterns; {} skipped", p.name));
        } else if out.items.iter().any(|q| q.name.eq_ignore_ascii_case(&p.name)) {
            out.warn(at, format!("pattern {} is defined twice; the first definition is kept", p.name));
        } else {
            out.items.push(p);
        }
    };
    for (no, line) in lines(text) {
        // A comment runs to the end of the line.
        let line = line.split(';').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if line.chars().count() > MAX_LINE_CHARS {
            out.warn(no, "line too long; skipped");
            continue;
        }
        if let Some(rest) = line.strip_prefix('*') {
            finish(cur.take(), &mut out);
            match header(rest) {
                Ok((name, description)) => cur = Some((UserPattern { name: name.to_ascii_uppercase(), description, lines: Vec::new() }, no, false)),
                Err(e) => out.warn(no, format!("{e}; skipped")),
            }
            continue;
        }
        let Some((p, _, failed)) = cur.as_mut() else {
            out.warn(no, "line family without a *NAME line before it; skipped");
            continue;
        };
        if *failed {
            continue;
        }
        if p.lines.len() >= MAX_PATTERN_FAMILIES {
            out.warn(no, format!("pattern {}: more than {MAX_PATTERN_FAMILIES} line families; skipped", p.name));
            *failed = true;
            continue;
        }
        match pat_family(line) {
            Ok(l) => p.lines.push(l),
            Err(e) => {
                out.warn(no, format!("pattern {}: {e}; skipped", p.name));
                *failed = true;
            }
        }
    }
    finish(cur, &mut out);
    out
}

// ---------- writing ----------

/// A number as the files write it: as short as it reads back exactly.
fn num(v: f64) -> String {
    if v == 0.0 { "0".into() } else { format!("{v}") }
}

/// A `*NAME,description` header line (commas in the description would end it early).
fn head(name: &str, description: &str) -> std::result::Result<String, String> {
    let name = name.trim();
    if name.is_empty() || name.contains([',', '*', ';', '\n', '\r']) {
        return Err(format!("`{name}` can't be a name in the file"));
    }
    let d = description.replace([',', '\n', '\r'], " ");
    Ok(if d.trim().is_empty() { format!("*{name}\n") } else { format!("*{name},{}\n", d.trim()) })
}

/// The `S=`, `R=`/`A=`, `X=`, `Y=` options of a text or shape element.
fn element_options(e: &DashElement) -> String {
    let mut o = format!(",S={}", num(if e.scale.is_finite() && e.scale > 0.0 { e.scale } else { 1.0 }));
    if e.absolute {
        o += &format!(",A={}", num(e.rotation.to_degrees()));
    } else if e.rotation != 0.0 && e.rotation.is_finite() {
        o += &format!(",R={}", num(e.rotation.to_degrees()));
    }
    if e.offset.x != 0.0 {
        o += &format!(",X={}", num(e.offset.x));
    }
    if e.offset.y != 0.0 {
        o += &format!(",Y={}", num(e.offset.y));
    }
    o
}

/// A `.lin` or `.pat` file's `text` with the definition named `name` replaced by `block` (its
/// `*NAME` line and the lines under it, as [`write_lin`] / [`write_pat`] write them), or `block`
/// added at the end. Every other line stays as it was, byte for byte: comments, blank lines,
/// definitions the readers skip. Of two definitions with the name, the first (the one the
/// readers keep) is replaced; a definition ends at the next `*` header or EOF (matching
/// [`parse_lin`] / [`parse_pat`]). Within the old span, data lines are replaced by `block`
/// and comment / blank lines are kept right after the new data.
pub fn put_definition(text: &str, name: &str, block: &str) -> String {
    let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let block: String = block.lines().filter(|l| !l.trim().is_empty() && !l.starts_with(';')).map(|l| format!("{l}{nl}")).collect();
    let (mut start, mut end) = (None::<usize>, None::<usize>);
    let mut found = false;
    let mut pos = 0usize;
    for line in text.split_inclusive('\n') {
        let t = line.trim();
        if found {
            if t.starts_with('*') {
                end = Some(pos);
                break;
            }
        } else if let Some(rest) = t.strip_prefix('*')
            && header(rest).is_ok_and(|(n, _)| n.eq_ignore_ascii_case(name))
        {
            start = Some(pos);
            found = true;
        }
        pos += line.len();
    }
    match start {
        Some(a) => {
            let b = end.unwrap_or(text.len());
            // Keep blank and comment lines from the old span (skip the header line).
            let kept: String = text
                .get(a..b)
                .unwrap_or_default()
                .split_inclusive('\n')
                .skip(1)
                .filter(|l| {
                    let t = l.trim();
                    t.is_empty() || t.starts_with(';')
                })
                .collect();
            format!("{}{block}{kept}{}", text.get(..a).unwrap_or_default(), text.get(b..).unwrap_or_default())
        }
        None if text.is_empty() || text.ends_with('\n') => format!("{text}{block}"),
        None => format!("{text}{nl}{block}"),
    }
}

/// Linetypes as a `.lin` file ([`parse_lin`] reads them back): `*NAME,description`, then
/// `A,` and each element's length, a text or shape in brackets after the element it ends.
/// A shape needs its name (`DashElement::shape_name`).
pub fn write_lin(linetypes: &[Linetype]) -> std::result::Result<String, String> {
    let mut out = String::from(";; Linetypes written by CADaddy\n");
    for lt in linetypes.iter().take(MAX_DEFINITIONS) {
        out += &head(&lt.name, &lt.description)?;
        let mut fields = vec!["A".to_string()];
        for e in lt.pattern.iter().take(MAX_LINETYPE_ELEMENTS) {
            if !e.length.is_finite() {
                return Err(format!("linetype {}: a length that isn't a number", lt.name));
            }
            fields.push(num(e.length));
            if let Some(t) = &e.text {
                if t.contains(['"', '\n', '\r']) {
                    return Err(format!("linetype {}: text `{t}` can't be written in a .lin file", lt.name));
                }
                fields.push(format!("[\"{t}\",{}{}]", e.style.as_deref().unwrap_or("STANDARD"), element_options(e)));
            } else if e.shape.is_some() || e.shape_name.is_some() {
                let Some(shape) = e.shape_name.as_deref() else {
                    return Err(format!("linetype {}: shape {} has no name (its shape file wasn't found)", lt.name, e.shape.unwrap_or(0)));
                };
                fields.push(format!("[{shape},{}{}]", e.style.as_deref().unwrap_or(super::SHAPE_FILE), element_options(e)));
            }
        }
        out += &fields.join(",");
        out.push('\n');
    }
    Ok(out)
}

/// Hatch patterns as a `.pat` file ([`parse_pat`] reads them back): `*NAME, description`, then a
/// line per family.
pub fn write_pat(patterns: &[UserPattern]) -> std::result::Result<String, String> {
    let mut out = String::from(";; Hatch patterns written by CADaddy\n");
    for p in patterns.iter().take(MAX_DEFINITIONS) {
        out += &head(&p.name, &p.description)?;
        for l in p.lines.iter().take(MAX_PATTERN_FAMILIES) {
            let mut v = vec![l.angle, l.origin.0, l.origin.1, l.delta.0, l.delta.1];
            v.extend(l.dashes.iter().take(MAX_PATTERN_DASHES));
            if v.iter().any(|x| !x.is_finite()) {
                return Err(format!("pattern {}: a value that isn't a number", p.name));
            }
            out += &v.iter().map(|x| num(*x)).collect::<Vec<_>>().join(", ");
            out.push('\n');
        }
    }
    Ok(out)
}

/// One line family: `angle, x-origin, y-origin, delta-x, delta-y[, dash-1, dash-2, …]`.
fn pat_family(line: &str) -> std::result::Result<PatternLine, String> {
    let mut v = Vec::new();
    for f in line.split(',') {
        if v.len() >= MAX_PATTERN_DASHES.saturating_add(5) {
            return Err(format!("more than {MAX_PATTERN_DASHES} dashes in a line family"));
        }
        let f = f.trim();
        if f.is_empty() && v.len() >= 5 {
            continue;
        }
        v.push(number(f).ok_or_else(|| format!("`{f}` is not a number"))?);
    }
    let (Some(&angle), Some(&x), Some(&y), Some(&dx), Some(&dy)) = (v.first(), v.get(1), v.get(2), v.get(3), v.get(4)) else {
        return Err("a line family needs angle, x, y, delta-x and delta-y".into());
    };
    if dy.abs() < 1e-9 {
        return Err("a line family with zero spacing (delta-y)".into());
    }
    Ok(PatternLine { angle, origin: (x, y), delta: (dx, dy), dashes: v.get(5..).unwrap_or(&[]).to_vec() })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One definition swapped in a file with Windows line ends: the rest kept byte for byte,
    /// the new lines written with the file's own line ends; a file without a final line end
    /// gets one before an added definition.
    #[test]
    fn a_definition_is_put_into_the_text_as_it_stands() {
        let crlf = ";; note\r\n*A,one\r\nA,1,-1\r\n\r\n*B,two\r\nA,2,-2\r\n";
        assert_eq!(put_definition(crlf, "a", ";; header\n*A,new\nA,3,-3\n"), ";; note\r\n*A,new\r\nA,3,-3\r\n\r\n*B,two\r\nA,2,-2\r\n");
        assert_eq!(put_definition("*B,two\nA,2,-2", "C", "*C\nA,1,-1\n"), "*B,two\nA,2,-2\n*C\nA,1,-1\n");
        assert_eq!(put_definition("", "C", "*C\nA,1,-1\n"), "*C\nA,1,-1\n");
    }

    /// A comment and blank line inside a definition belong to that definition (the readers
    /// skip them). Redefining the entry removes only its data lines; comments and blank lines
    /// stay, and the next definition is untouched.
    #[test]
    fn put_definition_keeps_comments_inside_the_old_block() {
        // .pat case: BOND has a comment and blank line between two families.
        let pat = "*BOND,old\n0, 0,0, 0,2\n; joints\n\n90, 0,0, 0,2\n*NEXT,next\n0, 0,0, 0,1\n";
        let out = put_definition(pat, "BOND", "*BOND,New bond\n0, 0,0, 0,3\n");
        assert_eq!(out, "*BOND,New bond\n0, 0,0, 0,3\n; joints\n\n*NEXT,next\n0, 0,0, 0,1\n");
        // parse_pat reads back BOND with exactly the one new family.
        let parsed = parse_pat(&out);
        let bond = parsed.items.iter().find(|p| p.name == "BOND").expect("BOND in parse_pat");
        assert_eq!(bond.lines.len(), 1, "only the new family");
        assert_eq!(bond.lines[0].delta.1, 3.0);
        let next = parsed.items.iter().find(|p| p.name == "NEXT").expect("NEXT in parse_pat");
        assert_eq!(next.lines.len(), 1);

        // .lin case: FENCE has a comment between *NAME and the pattern line.
        let lin = "*FENCE,old\n; note\nA,1,-0.5\n";
        let out = put_definition(lin, "FENCE", "*FENCE,New fence\nA,0.6,-0.2\n");
        assert_eq!(out, "*FENCE,New fence\nA,0.6,-0.2\n; note\n");
        // parse_lin reads FENCE with the new pattern and no warning about the comment.
        let parsed = parse_lin(&out);
        let fence = parsed.items.iter().find(|l| l.name == "FENCE").expect("FENCE in parse_lin");
        assert_eq!(fence.pattern.iter().map(|e| e.length).collect::<Vec<_>>(), [0.6, -0.2]);
        assert!(!parsed.warnings.iter().any(|w| w.contains("without a *NAME")), "no stray-line warning");
    }
}
