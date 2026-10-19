//! Binary plot style table files (`.ctb` colour-dependent, `.stb` named), read and written so
//! users' own tables load into the Plot Style Manager and ours open elsewhere.
//!
//! Implemented from public descriptions of the format: a short text header naming the format
//! (`PIAFILEVERSION_2.0,CTBVER1,compress` or `…STBVER1…`, a line break and `pmzlibcodec`), three
//! little-endian 32-bit numbers (a checksum, the text's length, the compressed length), then the
//! table as zlib-compressed text. The text is `key=value` lines and `name{ … }` blocks: the
//! table's description and flags, `plot_style{ 0{ … } 1{ … } … }` with one block per style, and a
//! `custom_lineweight_table{ … }` of millimetre widths the styles' lineweights index. A string
//! value starts with a `"` and runs to the end of its line.
//!
//! Field values: `color` is a 32-bit number whose top byte says how to read it (`0xC2`: the true
//! colour in its low three bytes; anything else: use the object's colour); `color_policy` bit 1
//! dithers and bit 2 prints grey; `screen` is the ink percentage; `linetype` 31 and `lineweight`
//! 0 use the object's; `end_style` 0–3 butt, square, round, diamond (4: object's); `join_style`
//! 0–3 miter, bevel, round, diamond (5: object's); `fill_style` 64–72 solid … vertical bars
//! (73: object's).
//!
//! Tests only use files this code writes.

use cadaddy_color::Rgb;
use cadaddy_doc::{EndStyle, FillStyle, JoinStyle, PlotStyle, PlotStyleKind, PlotStyleTable};

/// Largest compressed or uncompressed table we handle.
const MAX_BYTES: usize = 4 << 20;
const CODEC: &[u8] = b"pmzlibcodec";

/// Lineweights a table indexes, in millimetres (the standard series).
const LINEWEIGHTS_MM: [f64; 27] = [
    0.0, 0.05, 0.09, 0.10, 0.13, 0.15, 0.18, 0.20, 0.25, 0.30, 0.35, 0.40, 0.45, 0.50, 0.53, 0.60, 0.65, 0.70, 0.80, 0.90, 1.00, 1.06, 1.20, 1.40,
    1.58, 2.00, 2.11,
];

/// The linetype a style index stands for (31: the object's), by the names the format lists.
const LINETYPES: [&str; 31] = [
    "Solid",
    "Dashed",
    "Dotted",
    "Dash Dot",
    "Short Dash",
    "Medium Dash",
    "Long Dash",
    "Short Dash x2",
    "Medium Dash x2",
    "Long Dash x2",
    "Medium Long Dash",
    "Medium Dash Short Dash Short Dash",
    "Long Dash Short Dash",
    "Long Dash Dot Dot",
    "Long Dash Dot",
    "Medium Dash Dot Short Dash Dot",
    "Sparse Dot",
    "ISO Dash",
    "ISO Dash Space",
    "ISO Long Dash Dot",
    "ISO Long Dash Double Dot",
    "ISO Long Dash Triple Dot",
    "ISO Dot",
    "ISO Long Dash Short Dash",
    "ISO Long Dash Double Short Dash",
    "ISO Dash Dot",
    "ISO Double Dash Dot",
    "ISO Dash Double Dot",
    "ISO Double Dash Double Dot",
    "ISO Dash Triple Dot",
    "ISO Double Dash Triple Dot",
];
const OBJECT_LINETYPE: i64 = 31;
/// "Use the object's colour".
const OBJECT_COLOR: u32 = 0xC3FF_FFFF;

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for chunk in data.chunks(5552) {
        for &x in chunk {
            a += u32::from(x);
            b += a;
        }
        a %= 65521;
        b %= 65521;
    }
    (b << 16) | a
}

/// A string value as the format writes it (a leading quote, the line's rest).
fn text_value(s: &str) -> String {
    let clean: String = s.chars().map(|c| if c == '\n' || c == '\r' { ' ' } else { c }).collect();
    format!("\"{clean}")
}

fn end_code(e: EndStyle) -> i64 {
    match e {
        EndStyle::Butt => 0,
        EndStyle::Square => 1,
        EndStyle::Round => 2,
        EndStyle::Diamond => 3,
        EndStyle::Object => 4,
    }
}

fn join_code(j: JoinStyle) -> i64 {
    match j {
        JoinStyle::Miter => 0,
        JoinStyle::Bevel => 1,
        JoinStyle::Round => 2,
        JoinStyle::Diamond => 3,
        JoinStyle::Object => 5,
    }
}

const FILLS: [FillStyle; 9] = [
    FillStyle::Solid,
    FillStyle::Checkerboard,
    FillStyle::Crosshatch,
    FillStyle::Diamonds,
    FillStyle::HorizontalBars,
    FillStyle::SlantLeft,
    FillStyle::SlantRight,
    FillStyle::SquareDots,
    FillStyle::VerticalBars,
];

fn fill_code(f: FillStyle) -> i64 {
    FILLS.iter().position(|x| *x == f).map_or(73, |i| 64 + i as i64)
}

/// The table's text body.
pub fn body(t: &PlotStyleTable) -> String {
    let ctb = t.kind == PlotStyleKind::ColorDependent;
    let mut s = String::new();
    s.push_str(&format!("description={}\n", text_value(&t.description)));
    s.push_str(&format!("aci_table_available={}\n", if ctb { "TRUE" } else { "FALSE" }));
    s.push_str("scale_factor=1.0\napply_factor=FALSE\ncustom_lineweight_display_units=0\n");
    s.push_str("plot_style{\n");
    for (i, st) in t.styles.iter().enumerate() {
        let color = st.color.map_or(OBJECT_COLOR, |c| 0xC200_0000 | (u32::from(c.0) << 16) | (u32::from(c.1) << 8) | u32::from(c.2)) as i32;
        let lt = st.linetype.as_deref().and_then(|n| LINETYPES.iter().position(|x| x.eq_ignore_ascii_case(n))).map_or(OBJECT_LINETYPE, |i| i as i64);
        let lw = st
            .lineweight
            .map(|w| {
                let mm = f64::from(w) / 100.0;
                LINEWEIGHTS_MM.iter().enumerate().skip(1).min_by(|a, b| (a.1 - mm).abs().total_cmp(&(b.1 - mm).abs())).map_or(0, |(k, _)| k)
            })
            .unwrap_or(0);
        s.push_str(&format!(" {i}{{\n"));
        s.push_str(&format!(
            "  name={}\n  localized_name={}\n  description={}\n",
            text_value(&st.name),
            text_value(&st.name),
            text_value(&st.description)
        ));
        s.push_str(&format!("  color={color}\n  mode_color={color}\n  color_policy={}\n", 1 | if st.grayscale { 2 } else { 0 }));
        s.push_str("  physical_pen_number=0\n  virtual_pen_number=0\n");
        s.push_str(&format!(
            "  screen={}\n  linepattern_size=0.5\n  linetype={lt}\n  adaptive_linetype=TRUE\n  lineweight={lw}\n",
            st.screening.min(100)
        ));
        s.push_str(&format!("  fill_style={}\n  end_style={}\n  join_style={}\n }}\n", fill_code(st.fill), end_code(st.end), join_code(st.join)));
    }
    s.push_str("}\ncustom_lineweight_table{\n");
    for (i, mm) in LINEWEIGHTS_MM.iter().enumerate() {
        s.push_str(&format!(" {i}={mm:.2}\n"));
    }
    s.push_str("}\n");
    s
}

/// The table as a `.ctb` / `.stb` file.
pub fn write(t: &PlotStyleTable) -> Vec<u8> {
    let text = body(t);
    let packed = miniz_oxide::deflate::compress_to_vec_zlib(text.as_bytes(), 6);
    let kind = if t.kind == PlotStyleKind::ColorDependent { "CTBVER1" } else { "STBVER1" };
    let mut out = format!("PIAFILEVERSION_2.0,{kind},compress\r\n").into_bytes();
    out.extend_from_slice(CODEC);
    out.extend_from_slice(&adler32(text.as_bytes()).to_le_bytes());
    out.extend_from_slice(&u32::try_from(text.len()).unwrap_or(u32::MAX).to_le_bytes());
    out.extend_from_slice(&u32::try_from(packed.len()).unwrap_or(u32::MAX).to_le_bytes());
    out.extend_from_slice(&packed);
    out
}

/// Whether `bytes` look like a binary plot style table.
pub fn is_pia(bytes: &[u8]) -> bool {
    bytes.starts_with(b"PIAFILEVERSION")
}

/// The text inside a `.ctb` / `.stb` file, and whether it is colour-dependent.
fn unpack(bytes: &[u8]) -> Result<(String, bool), String> {
    if !is_pia(bytes) {
        return Err("not a plot style table file (no PIAFILEVERSION header)".into());
    }
    let head = bytes.get(..bytes.len().min(80)).unwrap_or_default();
    let ctb = head.windows(3).any(|w| w == b"CTB");
    let at = bytes.windows(CODEC.len()).position(|w| w == CODEC).ok_or("no pmzlibcodec marker")?;
    let data = bytes.get(at + CODEC.len() + 12..).ok_or("the file ends in its header")?;
    let raw = miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(data, MAX_BYTES)
        .or_else(|_| miniz_oxide::inflate::decompress_to_vec_with_limit(data.get(2..).unwrap_or_default(), MAX_BYTES))
        .map_err(|_| "the table's compressed text is damaged".to_string())?;
    Ok((String::from_utf8_lossy(&raw).into_owned(), ctb))
}

/// A parsed `key=value` / `name{ … }` tree.
#[derive(Debug, Default)]
struct Node {
    values: Vec<(String, String)>,
    children: Vec<(String, Node)>,
}

impl Node {
    fn get(&self, k: &str) -> Option<&str> {
        self.values.iter().find(|(kk, _)| kk == k).map(|(_, v)| v.as_str())
    }
    fn child(&self, k: &str) -> Option<&Node> {
        self.children.iter().find(|(kk, _)| kk == k).map(|(_, n)| n)
    }
}

fn parse(text: &str) -> Node {
    let mut stack: Vec<(String, Node)> = vec![(String::new(), Node::default())];
    for line in text.lines().take(200_000) {
        let l = line.trim();
        if l.is_empty() {
            continue;
        }
        if l == "}" {
            if stack.len() > 1
                && let Some((name, node)) = stack.pop()
                && let Some(top) = stack.last_mut()
            {
                top.1.children.push((name, node));
            }
            continue;
        }
        if let Some(name) = l.strip_suffix('{') {
            if stack.len() < 16 {
                stack.push((name.trim().to_string(), Node::default()));
            }
            continue;
        }
        if let Some((k, v)) = l.split_once('=')
            && let Some(top) = stack.last_mut()
        {
            top.1.values.push((k.trim().to_string(), v.trim_start_matches('"').to_string()));
        }
    }
    while stack.len() > 1 {
        if let Some((name, node)) = stack.pop()
            && let Some(top) = stack.last_mut()
        {
            top.1.children.push((name, node));
        }
    }
    stack.pop().map(|(_, n)| n).unwrap_or_default()
}

/// A `.ctb` / `.stb` file as a table named `name`.
pub fn read(bytes: &[u8], name: &str) -> Result<PlotStyleTable, String> {
    if bytes.len() > MAX_BYTES {
        return Err("the plot style table file is too large".into());
    }
    let (text, ctb_header) = unpack(bytes)?;
    let root = parse(&text);
    let ctb = root.get("aci_table_available").map_or(ctb_header, |v| v.eq_ignore_ascii_case("TRUE"));
    let num = |n: &Node, k: &str| n.get(k).and_then(|v| v.trim().parse::<f64>().ok()).filter(|v| v.is_finite());
    let weights: Vec<f64> = root
        .child("custom_lineweight_table")
        .map(|t| {
            let mut v: Vec<(usize, f64)> = t.values.iter().filter_map(|(k, x)| Some((k.parse().ok()?, x.trim().parse().ok()?))).collect();
            v.sort_by_key(|(k, _)| *k);
            v.into_iter().map(|(_, x)| x).take(256).collect()
        })
        .filter(|v: &Vec<f64>| !v.is_empty())
        .unwrap_or_else(|| LINEWEIGHTS_MM.to_vec());
    let mut styles = Vec::new();
    if let Some(ps) = root.child("plot_style") {
        let mut blocks: Vec<(usize, &Node)> = ps.children.iter().filter_map(|(k, n)| Some((k.parse().ok()?, n))).collect();
        blocks.sort_by_key(|(k, _)| *k);
        for (_, n) in blocks.into_iter().take(cadaddy_doc::MAX_NAMED_STYLES.max(cadaddy_doc::CTB_STYLES)) {
            let color = num(n, "color").map(|c| c as i64 as u32).filter(|c| c >> 24 == 0xC2).map(|c| Rgb((c >> 16) as u8, (c >> 8) as u8, c as u8));
            let policy = num(n, "color_policy").unwrap_or(1.0) as i64;
            let lt = num(n, "linetype").map_or(OBJECT_LINETYPE, |v| v as i64);
            let lw = num(n, "lineweight").map_or(0, |v| v as i64);
            let code = |k: &str, default: i64| num(n, k).map_or(default, |v| v as i64);
            styles.push(PlotStyle {
                name: n.get("name").unwrap_or_default().chars().take(255).collect(),
                description: n.get("description").unwrap_or_default().chars().take(1024).collect(),
                color,
                grayscale: policy & 2 != 0,
                screening: num(n, "screen").map_or(100, |v| v.clamp(0.0, 100.0) as u8),
                linetype: usize::try_from(lt).ok().and_then(|i| LINETYPES.get(i)).map(|s| s.to_string()),
                lineweight: usize::try_from(lw)
                    .ok()
                    .filter(|i| *i > 0)
                    .and_then(|i| weights.get(i))
                    .map(|mm| (mm * 100.0).round().clamp(0.0, 211.0) as u16),
                end: match code("end_style", 4) {
                    0 => EndStyle::Butt,
                    1 => EndStyle::Square,
                    2 => EndStyle::Round,
                    3 => EndStyle::Diamond,
                    _ => EndStyle::Object,
                },
                join: match code("join_style", 5) {
                    0 => JoinStyle::Miter,
                    1 => JoinStyle::Bevel,
                    2 => JoinStyle::Round,
                    3 => JoinStyle::Diamond,
                    _ => JoinStyle::Object,
                },
                fill: usize::try_from(code("fill_style", 73) - 64).ok().and_then(|i| FILLS.get(i).copied()).unwrap_or(FillStyle::Object),
            });
        }
    }
    let kind = if ctb { PlotStyleKind::ColorDependent } else { PlotStyleKind::Named };
    let t = PlotStyleTable {
        name: name.chars().take(255).collect(),
        description: root.get("description").unwrap_or_default().chars().take(1024).collect(),
        kind,
        styles,
    };
    Ok(t.sanitized())
}
