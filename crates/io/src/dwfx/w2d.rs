//! The W2D graphics stream of a DWF 6 file, written and read with the readable (ASCII) opcodes
//! of Autodesk's WHIP!/DWF data format, as documented in "DWF format" (Autodesk's DWF/WHIP!
//! specification, edited by Paul Bourke, paulbourke.net/dataformats/whip/). Every opcode used
//! is traced to its section there:
//! - file header `(DWF V06.00)` and trailer `(EndOfDWF)`: the index page's "File header" and
//!   "End of DWF" paragraphs (the header is `(DWF V` and a five-byte `MM.mm` version);
//! - `(Layer <num> '<name>')`: "Set Layer" (extended ASCII; the name in single quotes as its
//!   `'Default Layer'` example);
//! - `(Color <r>,<g>,<b>,<a>)`: "Set Color" (extended ASCII RGBA form);
//! - `F` / `f`: "Set Fill Mode" (fill on: the next polygon is filled; off: outlines);
//! - `P <count> <x>,<y> …`: "Draw Polyline/Polygon" (ASCII form, absolute logical coordinates;
//!   the count is the number of points; with fill on, a polygon of that many sides).
//!
//! Logical coordinates are integers, [`PER_INCH`] to the sheet inch, y up, from the sheet's
//! lower-left corner. The reader reads exactly these opcodes (and skips other extended ASCII
//! ones); a stream using the binary opcodes is refused with a message naming DWFx.

use cadaddy_color::Rgb;
use cadaddy_geom::Vec2;

/// Logical units per sheet inch.
pub(crate) const PER_INCH: f64 = 10_000.0;
/// The stream's header (WHIP! "File header": `(DWF V` + version).
pub(crate) const HEADER: &[u8; 12] = b"(DWF V06.00)";
const TRAILER: &str = "(EndOfDWF)";
/// Most points one stream holds when read (hostile files).
const MAX_POINTS: usize = 8_000_000;

/// What a stream draws, in sheet inches.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Shape {
    /// An open or closed polyline.
    Line(Vec<Vec2>),
    /// A filled polygon.
    Fill(Vec<Vec2>),
}

/// One thing drawn: its layer, colour and shape.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Drawn {
    pub layer: String,
    pub rgb: Rgb,
    pub shape: Shape,
}

fn logical(v: f64) -> i64 {
    let x = (v * PER_INCH).round();
    if x.is_finite() { x.clamp(-2_147_483_647.0, 2_147_483_647.0) as i64 } else { 0 }
}

/// A layer name for `(Layer n '…')`: printable, without the quote and parentheses that end it.
fn layer_text(name: &str) -> String {
    name.chars().map(|c| if c == '\'' || c == '(' || c == ')' || c.is_control() { '_' } else { c }).collect()
}

/// The stream for `items` (sheet inches).
pub(crate) fn write(items: &[Drawn]) -> Vec<u8> {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(items.len() * 48 + 64);
    s.push_str(std::str::from_utf8(HEADER).unwrap_or("(DWF V06.00)"));
    s.push('\n');
    let mut layers: Vec<&str> = Vec::new();
    let (mut layer, mut color, mut fill): (Option<usize>, Option<Rgb>, bool) = (None, None, false);
    for it in items {
        let n = match layers.iter().position(|l| *l == it.layer) {
            Some(i) => i,
            None => {
                layers.push(&it.layer);
                layers.len() - 1
            }
        };
        if layer != Some(n) {
            // Layer 0 is WHIP!'s always-visible default layer: drawing layers start at 1.
            let _ = writeln!(s, "(Layer {} '{}')", n + 1, layer_text(&it.layer));
            layer = Some(n);
        }
        if color != Some(it.rgb) {
            let _ = writeln!(s, "(Color {},{},{},255)", it.rgb.0, it.rgb.1, it.rgb.2);
            color = Some(it.rgb);
        }
        let (pts, want_fill) = match &it.shape {
            Shape::Line(p) => (p, false),
            Shape::Fill(p) => (p, true),
        };
        if pts.len() < 2 || (want_fill && pts.len() < 3) {
            continue;
        }
        if fill != want_fill {
            s.push_str(if want_fill { "F\n" } else { "f\n" });
            fill = want_fill;
        }
        let _ = write!(s, "P {}", pts.len());
        for p in pts {
            let _ = write!(s, " {},{}", logical(p.x), logical(p.y));
        }
        s.push('\n');
    }
    s.push_str(TRAILER);
    s.push('\n');
    s.into_bytes()
}

/// The message for a stream CADaddy can't read.
fn unsupported(what: String) -> String {
    format!(
        "{what}: CADaddy reads DWF 6 files written with WHIP!'s readable opcodes (as its own DWFOUT writes them); save the sheet set as DWFx and attach that"
    )
}

/// What a stream draws (sheet inches).
pub(crate) fn read(data: &[u8]) -> Result<Vec<Drawn>, String> {
    let text =
        data.strip_prefix(b"(DWF V".as_slice()).or_else(|| data.strip_prefix(b"(W2D V".as_slice())).ok_or("not a W2D stream (no `(DWF V` header)")?;
    let text = std::str::from_utf8(text).map_err(|_| unsupported("the stream uses binary opcodes".into()))?;
    let b = text.as_bytes();
    // Past the header's version and `)`.
    let mut i = text.find(')').map(|e| e + 1).ok_or("a bad W2D header")?;
    let mut out = Vec::new();
    let mut layers: Vec<(i64, String)> = Vec::new();
    let (mut layer, mut rgb, mut fill) = ("0".to_string(), Rgb(0, 0, 0), false);
    let mut total = 0usize;
    let num = |i: &mut usize| -> Option<i64> {
        while b.get(*i).is_some_and(u8::is_ascii_whitespace) {
            *i += 1;
        }
        let start = *i;
        if matches!(b.get(*i), Some(b'-' | b'+')) {
            *i += 1;
        }
        while b.get(*i).is_some_and(u8::is_ascii_digit) {
            *i += 1;
        }
        text.get(start..*i)?.parse::<i64>().ok()
    };
    while let Some(&c) = b.get(i) {
        match c {
            c if c.is_ascii_whitespace() => i += 1,
            b'F' => {
                fill = true;
                i += 1;
            }
            b'f' => {
                fill = false;
                i += 1;
            }
            b'P' => {
                i += 1;
                let n = num(&mut i).and_then(|n| usize::try_from(n).ok()).filter(|n| *n >= 1).ok_or("a `P` without a point count")?;
                total = total.saturating_add(n);
                if total > MAX_POINTS {
                    return Err("the W2D stream has too many points".into());
                }
                let mut pts = Vec::with_capacity(n.min(65_536));
                for _ in 0..n {
                    let x = num(&mut i).ok_or("a `P` with too few points")?;
                    if b.get(i) != Some(&b',') {
                        return Err("a `P` point without `,`".into());
                    }
                    i += 1;
                    let y = num(&mut i).ok_or("a `P` with too few points")?;
                    pts.push(Vec2::new(x as f64 / PER_INCH, y as f64 / PER_INCH));
                }
                let shape = if fill && pts.len() >= 3 { Shape::Fill(pts) } else { Shape::Line(pts) };
                out.push(Drawn { layer: layer.clone(), rgb, shape });
            }
            b'(' => {
                // An extended ASCII opcode: its name, then operands up to the matching `)`.
                let mut depth = 0usize;
                let mut j = i;
                let mut quoted = false;
                while let Some(&d) = b.get(j) {
                    match d {
                        b'\'' => quoted = !quoted,
                        b'(' if !quoted => depth += 1,
                        b')' if !quoted => {
                            depth = depth.saturating_sub(1);
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    j += 1;
                }
                let op = text.get(i + 1..j).ok_or("an extended opcode is not closed")?;
                if b.get(j).is_none() {
                    return Err("an extended opcode is not closed".into());
                }
                i = j + 1;
                let (name, args) = op.split_once(|ch: char| ch.is_ascii_whitespace()).unwrap_or((op, ""));
                match name {
                    "EndOfDWF" => break,
                    "Layer" => {
                        let args = args.trim();
                        let (n, rest) = args.split_once(|ch: char| ch.is_ascii_whitespace()).unwrap_or((args, ""));
                        let n = n.parse::<i64>().map_err(|_| "a `(Layer` without a number")?;
                        let rest = rest.trim();
                        let named = rest.strip_prefix('\'').and_then(|r| r.strip_suffix('\'')).unwrap_or(rest);
                        match layers.iter().find(|(k, _)| *k == n) {
                            Some((_, name)) if named.is_empty() => layer = name.clone(),
                            _ => {
                                layer = if named.is_empty() { if n == 0 { "0".into() } else { format!("Layer {n}") } } else { named.to_string() };
                                layers.retain(|(k, _)| *k != n);
                                if layers.len() < 32_768 {
                                    layers.push((n, layer.clone()));
                                }
                            }
                        }
                    }
                    "Color" => {
                        let v: Vec<u8> = args.split(',').filter_map(|t| t.trim().parse::<u8>().ok()).collect();
                        if let [r, g, bl, ..] = v.as_slice() {
                            rgb = Rgb(*r, *g, *bl);
                        }
                    }
                    _ => {}
                }
            }
            other => return Err(unsupported(format!("unsupported W2D opcode 0x{other:02X}"))),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn our_stream_decodes_to_the_source_coordinates() {
        let items = vec![
            Drawn { layer: "WALLS".into(), rgb: Rgb(255, 0, 0), shape: Shape::Line(vec![Vec2::new(1.25, 2.5), Vec2::new(10.0001, 2.5)]) },
            Drawn {
                layer: "NOTES (2)".into(),
                rgb: Rgb(0, 0, 255),
                shape: Shape::Fill(vec![Vec2::new(0.0, 0.0), Vec2::new(1.0, 0.0), Vec2::new(0.5, 0.75)]),
            },
            Drawn { layer: "WALLS".into(), rgb: Rgb(255, 0, 0), shape: Shape::Line(vec![Vec2::new(3.0, 3.0), Vec2::new(4.0, 5.0)]) },
        ];
        let bytes = write(&items);
        let text = std::str::from_utf8(&bytes).unwrap();
        assert!(text.starts_with("(DWF V06.00)\n(Layer 1 'WALLS')\n(Color 255,0,0,255)\nP 2 12500,25000 100001,25000\n"), "{text}");
        assert!(text.contains("(Layer 2 'NOTES _2_')\n(Color 0,0,255,255)\nF\nP 3 0,0 10000,0 5000,7500\n"), "{text}");
        assert!(text.ends_with("(EndOfDWF)\n"));
        let back = read(&bytes).unwrap();
        assert_eq!(back.len(), 3);
        assert_eq!(
            back[0],
            Drawn { layer: "WALLS".into(), rgb: Rgb(255, 0, 0), shape: Shape::Line(vec![Vec2::new(1.25, 2.5), Vec2::new(10.0001, 2.5)]) }
        );
        assert_eq!(back[1].layer, "NOTES _2_");
        assert_eq!(back[1].shape, Shape::Fill(vec![Vec2::new(0.0, 0.0), Vec2::new(1.0, 0.0), Vec2::new(0.5, 0.75)]));
        assert_eq!(back[2].layer, "WALLS");
    }

    #[test]
    fn binary_and_broken_streams_are_refused() {
        let e = read(b"(DWF V06.00)\x10\x01\x02").unwrap_err();
        assert!(e.contains("unsupported W2D opcode 0x10") && e.contains("DWFx"), "{e}");
        assert!(read(b"PK\x03\x04").is_err());
        assert!(read(b"(DWF V06.00) P 3 1,1").is_err());
        assert!(read(b"(DWF V06.00) (Color 1,2,3").is_err());
        assert!(read(b"(DWF V06.00) P 99999999999 1,1").is_err());
    }
}
