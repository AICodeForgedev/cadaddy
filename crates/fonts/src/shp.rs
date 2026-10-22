//! Shape source files (`.shp`) and their compiler: the text form of a shape file, compiled to
//! the `.shx` form [`crate::shx`] reads (COMPILE, MKSHAPE).
//!
//! Written from the public description of shape definitions: each shape is a header line
//! `*number,bytes,NAME` followed by its specification bytes, separated by commas, over as many
//! lines as needed and ending with 0. A byte is decimal (`12`) or, with a leading zero,
//! hexadecimal (`012` is 0x12); parentheses may group bytes for reading (`8,(-2,3)`). A
//! negative displacement or bulge is a signed byte; a negative octant (codes 10 and 11) means
//! clockwise. `;` starts a comment. Shape 0, when present, holds the font information.
//! Unicode (`*UNIFONT`) and big font (`*BIGFONT`) sources aren't compiled yet.
//!
//! Every count is capped: these are files a user hands us.

use std::fmt::Write as _;

/// Most shapes one file defines.
const MAX_SHAPES: usize = 65_535;
/// Most bytes one shape's specification has (the compiled format's limit).
const MAX_BYTES: usize = 2000;
/// Longest source file read.
const MAX_SOURCE: usize = 16 << 20;
/// Longest shape name.
const MAX_NAME: usize = 128;

/// One shape: its number, name and specification bytes (ending with 0).
#[derive(Clone, Debug, PartialEq)]
pub struct ShapeDef {
    pub number: u16,
    pub name: String,
    pub spec: Vec<u8>,
}

/// A compiled source: the `.shx` bytes, the shapes in it, and notes about lines that were
/// questionable but compiled (a byte count that didn't match, for instance).
#[derive(Clone, Debug, Default)]
pub struct Compiled {
    pub shx: Vec<u8>,
    pub shapes: Vec<ShapeDef>,
    pub warnings: Vec<String>,
}

/// A source value: a number as written.
fn number(t: &str) -> Option<i32> {
    let t = t.trim();
    let (neg, body) = match t.strip_prefix('-') {
        Some(b) => (true, b.trim()),
        None => (false, t.strip_prefix('+').unwrap_or(t).trim()),
    };
    if body.is_empty() || body.len() > 8 {
        return None;
    }
    let v = if body.len() > 1 && body.starts_with('0') { i32::from_str_radix(body.get(1..)?, 16).ok()? } else { body.parse::<i32>().ok()? };
    Some(if neg { -v } else { v })
}

/// The values of a specification's lines: numbers, with parentheses and spaces ignored.
fn values(lines: &[(usize, String)]) -> Result<Vec<i32>, String> {
    let mut out = Vec::new();
    for (line, text) in lines {
        for t in text.split(',').map(|t| t.trim().trim_matches(|c| c == '(' || c == ')').trim()).filter(|t| !t.is_empty()) {
            out.push(number(t).ok_or_else(|| format!("line {line}: `{t}` is not a number"))?);
            if out.len() > MAX_BYTES {
                return Err(format!("line {line}: more than {MAX_BYTES} bytes"));
            }
        }
    }
    Ok(out)
}

/// An unsigned byte.
fn unsigned(v: i32, what: &str) -> Result<u8, String> {
    u8::try_from(v).map_err(|_| format!("{what} {v} is out of range (0 to 255)"))
}

/// A signed byte (displacements and bulges: -128 to 127).
fn signed(v: i32, what: &str) -> Result<u8, String> {
    i8::try_from(v).map(|b| b as u8).map_err(|_| format!("{what} {v} is out of range (-128 to 127)"))
}

/// An octant byte: its sign is the direction (negative: clockwise, the high bit).
fn octant(v: i32) -> Result<u8, String> {
    let m = unsigned(v.abs(), "octant")?;
    if m > 0x7f {
        return Err(format!("octant {v} is out of range"));
    }
    Ok(if v < 0 { m | 0x80 } else { m })
}

/// The bytes of a shape's specification from its values: every byte after a command code
/// read as that command takes it (signed displacements, octant directions).
fn encode(vals: &[i32], font_info: bool) -> Result<Vec<u8>, String> {
    if font_info {
        return vals.iter().map(|v| unsigned(*v, "value")).collect();
    }
    let mut out = Vec::with_capacity(vals.len());
    let mut i = 0;
    let at = |i: usize| vals.get(i).copied().ok_or_else(|| "the specification ends inside a command".to_string());
    while i < vals.len() {
        let code = unsigned(at(i)?, "code")?;
        out.push(code);
        i += 1;
        match code {
            0 => break,
            3 | 4 | 7 => {
                out.push(unsigned(at(i)?, "value")?);
                i += 1;
            }
            8 => {
                out.push(signed(at(i)?, "displacement")?);
                out.push(signed(at(i + 1)?, "displacement")?);
                i += 2;
            }
            9 | 13 => loop {
                let (x, y) = (at(i)?, at(i + 1)?);
                out.push(signed(x, "displacement")?);
                out.push(signed(y, "displacement")?);
                i += 2;
                if x == 0 && y == 0 {
                    break;
                }
                if code == 13 {
                    out.push(signed(at(i)?, "bulge")?);
                    i += 1;
                }
            },
            10 => {
                out.push(unsigned(at(i)?, "radius")?);
                out.push(octant(at(i + 1)?)?);
                i += 2;
            }
            11 => {
                for k in 0..4 {
                    out.push(unsigned(at(i + k)?, "value")?);
                }
                out.push(octant(at(i + 4)?)?);
                i += 5;
            }
            12 => {
                out.push(signed(at(i)?, "displacement")?);
                out.push(signed(at(i + 1)?, "displacement")?);
                out.push(signed(at(i + 2)?, "bulge")?);
                i += 3;
            }
            _ => {}
        }
        if out.len() > MAX_BYTES {
            return Err(format!("more than {MAX_BYTES} bytes"));
        }
    }
    if out.last() != Some(&0) {
        out.push(0);
    }
    Ok(out)
}

/// Compile shape source text to a `.shx` file. An error names the line at fault.
pub fn compile(text: &str) -> Result<Compiled, String> {
    if text.len() > MAX_SOURCE {
        return Err(format!("the source is larger than {} MB", MAX_SOURCE >> 20));
    }
    // (number, declared byte count, name, the header's line, the specification's lines so far).
    let mut open: Option<(u16, usize, String, usize, Vec<(usize, String)>)> = None;
    let mut out = Compiled::default();
    let mut seen = std::collections::HashSet::new();
    let mut finish = |o: (u16, usize, String, usize, Vec<(usize, String)>), out: &mut Compiled| -> Result<(), String> {
        let (number, declared, name, line, body) = o;
        let vals = values(&body)?;
        let spec = encode(&vals, number == 0).map_err(|e| format!("shape {number} ({name}), line {line}: {e}"))?;
        if spec.len() != declared {
            out.warnings.push(format!("shape {number} ({name}): the header says {declared} bytes, the specification has {}", spec.len()));
        }
        if !seen.insert(number) {
            return Err(format!("line {line}: shape {number} is defined twice"));
        }
        if out.shapes.len() >= MAX_SHAPES {
            return Err(format!("more than {MAX_SHAPES} shapes"));
        }
        out.shapes.push(ShapeDef { number, name, spec });
        Ok(())
    };
    for (k, raw) in text.lines().enumerate() {
        let line = k + 1;
        let l = raw.split(';').next().unwrap_or("").trim();
        if l.is_empty() {
            continue;
        }
        if let Some(head) = l.strip_prefix('*') {
            if let Some(o) = open.take() {
                finish(o, &mut out)?;
            }
            let mut parts = head.splitn(3, ',');
            let num = parts.next().unwrap_or("").trim();
            if num.eq_ignore_ascii_case("UNIFONT") || num.eq_ignore_ascii_case("BIGFONT") {
                return Err(format!("line {line}: {} sources can't be compiled yet", num.to_ascii_uppercase()));
            }
            let num_v = number(num).and_then(|n| u16::try_from(n).ok()).ok_or_else(|| format!("line {line}: `{num}` is not a shape number"))?;
            let declared = parts
                .next()
                .and_then(number)
                .and_then(|n| usize::try_from(n).ok())
                .ok_or_else(|| format!("line {line}: the byte count is missing"))?;
            let name: String = parts.next().unwrap_or("").trim().chars().take(MAX_NAME).collect();
            if num_v != 0 && name.is_empty() {
                out.warnings.push(format!("line {line}: shape {num_v} has no name"));
            }
            open = Some((num_v, declared, name.to_ascii_uppercase(), line, Vec::new()));
        } else {
            let Some(o) = open.as_mut() else { return Err(format!("line {line}: bytes before any `*number,bytes,NAME` header")) };
            o.4.push((line, l.to_string()));
        }
    }
    if let Some(o) = open.take() {
        finish(o, &mut out)?;
    }
    if out.shapes.iter().all(|s| s.number == 0) {
        return Err("the source defines no shapes".into());
    }
    let defs = out.shapes.iter().map(|s| (s.number, [s.name.as_bytes(), &[0], &s.spec].concat())).collect();
    out.shx = crate::shx::compile_defs(defs);
    Ok(out)
}

/// The source text of shapes, as COMPILE reads it (displacements signed, octants with their
/// direction as the sign), one shape per header and up to 16 bytes a line.
pub fn source(shapes: &[ShapeDef]) -> String {
    let mut out = String::new();
    for s in shapes {
        let _ = writeln!(out, "*{},{},{}", s.number, s.spec.len(), s.name);
        let vals = decode(&s.spec, s.number == 0);
        for chunk in vals.chunks(16) {
            let _ = writeln!(out, "{}", chunk.join(","));
        }
    }
    out
}

/// The source form of each byte of a specification (the inverse of [`encode`]).
fn decode(spec: &[u8], font_info: bool) -> Vec<String> {
    let u = |b: u8| b.to_string();
    let s = |b: u8| (b as i8).to_string();
    let o = |b: u8| if b & 0x80 != 0 { format!("-0{:X}", b & 0x7f) } else { format!("0{b:X}") };
    if font_info {
        return spec.iter().map(|b| u(*b)).collect();
    }
    let mut out = Vec::new();
    let mut i = 0;
    let at = |i: usize| spec.get(i).copied();
    while let Some(code) = at(i) {
        i += 1;
        out.push(if code > 14 { format!("0{code:X}") } else { u(code) });
        let mut take = |n: usize, f: &dyn Fn(u8) -> String, i: &mut usize| {
            for _ in 0..n {
                if let Some(b) = at(*i) {
                    out.push(f(b));
                }
                *i += 1;
            }
        };
        match code {
            0 => break,
            3 | 4 | 7 => take(1, &u, &mut i),
            8 => take(2, &s, &mut i),
            9 | 13 => {
                while let (Some(x), Some(y)) = (at(i), at(i + 1)) {
                    take(2, &s, &mut i);
                    if x == 0 && y == 0 {
                        break;
                    }
                    if code == 13 {
                        take(1, &s, &mut i);
                    }
                }
            }
            10 => {
                take(1, &u, &mut i);
                take(1, &o, &mut i);
            }
            11 => {
                take(4, &u, &mut i);
                take(1, &o, &mut i);
            }
            12 => take(3, &s, &mut i),
            _ => {}
        }
    }
    out
}

/// The shapes of a source, read without compiling (for replacing one and writing it back).
pub fn read(text: &str) -> Result<Vec<ShapeDef>, String> {
    compile(text).map(|c| c.shapes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shx::ShxFont;
    use cadaddy_geom::Vec2;

    const BOX: &str = "; a made-up shape file\n*1,5,BOX\n014,010,01C,018,0\n*2,9,DIAG\n2,8,(-2,3),1,8,(2,-3),0\n*3,6,QUARTER\n10,(2,-043),0\n";

    #[test]
    fn a_source_compiles_and_reads_back() {
        let c = compile(BOX).unwrap();
        let names: Vec<&str> = c.shapes.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["BOX", "DIAG", "QUARTER"]);
        // QUARTER says 6 bytes and has 4: noted, and compiled.
        assert_eq!(c.warnings.len(), 1, "{:?}", c.warnings);
        let f = ShxFont::parse(&c.shx).unwrap();
        assert_eq!(f.code_by_name("box"), Some(1));
        assert_eq!(f.shape_names(), vec![(1, "BOX".into()), (2, "DIAG".into()), (3, "QUARTER".into())]);
        // BOX: up 1, right 1, down 1, left 1 from the origin — a closed unit square.
        let g = f.glyph(1, false).unwrap();
        assert_eq!(g.strokes, vec![vec![Vec2::new(0.0, 0.0), Vec2::new(0.0, 1.0), Vec2::new(1.0, 1.0), Vec2::new(1.0, 0.0), Vec2::new(0.0, 0.0)]]);
        // DIAG: pen up to (-2, 3), down, back to the origin. Negative displacements are signed.
        let g = f.glyph(2, false).unwrap();
        assert_eq!(g.strokes, vec![vec![Vec2::new(-2.0, 3.0), Vec2::new(0.0, 0.0)]]);
        // QUARTER: a clockwise arc of radius 2 from octant 4 (west of its centre) through 3
        // octants: centre (2, 0), ending at 45°.
        let g = f.glyph(3, false).unwrap();
        let c = Vec2::new(2.0, 0.0);
        assert!(g.strokes.iter().flatten().all(|p| (p.dist(c) - 2.0).abs() < 1e-9), "{g:?}");
        assert!(g.advance.dist(c + Vec2::new(2.0f64.sqrt(), 2.0f64.sqrt())) < 1e-9, "{:?}", g.advance);
    }

    #[test]
    fn source_round_trips_through_its_text() {
        let c = compile(BOX).unwrap();
        let again = compile(&source(&c.shapes)).unwrap();
        assert_eq!(again.shapes, c.shapes);
        assert!(again.warnings.is_empty(), "{:?}", again.warnings);
        assert_eq!(again.shx, c.shx);
    }

    #[test]
    fn errors_name_the_line() {
        assert!(compile("*1,2,A\n0x9,0\n").unwrap_err().contains("line 2"));
        assert!(compile("014,0\n").unwrap_err().contains("line 1"));
        assert!(compile("*1,4,A\n8,200,0,0\n").unwrap_err().contains("out of range"));
        assert!(compile("*1,2,A\n0\n*1,2,B\n0\n").unwrap_err().contains("twice"));
        assert!(compile("*UNIFONT,6,X\n").unwrap_err().contains("UNIFONT"));
        assert!(compile("; nothing\n").is_err());
        // Shape 0 (the font information) takes its bytes as they are.
        let c = compile("*0,4,Made Up Font\n6,2,0,0\n*65,4,A\n014,0\n").unwrap();
        let f = ShxFont::parse(&c.shx).unwrap();
        assert_eq!((f.above, f.below, f.len()), (6.0, 2.0, 1));
    }

    #[test]
    fn hostile_sources_never_panic() {
        for t in [
            "*",
            "*,,",
            "*99999999,1,A\n0",
            "*1,1,A\n9,1",
            "*1,1,A\n13,1,2",
            "*1,1,A\n11,1,2",
            "*1,-5,A\n0",
            "*1,1,A\n((((,,,",
            "*1,1,A\n-0FFFFFFFFF",
        ] {
            let _ = compile(t);
        }
        let long = format!("*1,1,A\n{}", "1,".repeat(5000));
        assert!(compile(&long).is_err());
    }
}
