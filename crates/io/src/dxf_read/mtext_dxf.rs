//! MTEXT specifics of DXF (DXF Reference, MTEXT): control characters in the contents written
//! as a caret and a letter (`^I` a tab, `^J` a line feed, `^ ` a caret), and the column groups
//! 75 (type), 76 (count), 78 (flow reversed), 79 (auto height), 48 (width), 49 (gutter) and
//! 50 (column heights, after the column type; before it, 50 is the rotation).

use cadaddy_doc::{MText, MTextColumnType, MTextColumns};
use cadaddy_dxf::Tag;

/// Upper bound on column heights read from a file.
const MAX_COLUMNS: usize = 1_000;

/// Decode the contents' `^I` (tab), `^J` (line feed) and `^ ` (caret). Other carets stay as
/// they are (older files write stacked tolerances `\S+0.1^-0.1;` with a bare caret).
pub(crate) fn decode_contents(s: &str) -> String {
    if !s.contains('^') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c != '^' {
            out.push(c);
            continue;
        }
        match it.peek().copied() {
            Some(' ') => out.push('^'),
            Some('I') => out.push('\t'),
            Some('J') => out.push('\n'),
            _ => {
                out.push('^');
                continue;
            }
        }
        it.next();
    }
    out
}

/// Encoded contents split into the 250-character strings of MTEXT's 3 and 1 groups, never
/// between a caret and the character after it: readers that decode each string on its own
/// (acadrust, so DWG save) would keep a split `^I` as the two characters.
pub(crate) fn chunks(encoded: &str) -> Vec<String> {
    let chars: Vec<char> = encoded.chars().collect();
    let mut out = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let mut end = (start + 250).min(chars.len());
        if end < chars.len() && chars.get(end - 1) == Some(&'^') && end - 1 > start {
            end -= 1;
        }
        out.push(chars.get(start..end).unwrap_or_default().iter().collect());
        start = end;
    }
    out
}

/// Encode the contents for a DXF string: a tab as `^I`, a line feed as `^J`, a caret as `^ `;
/// other control characters (which draw nothing) are dropped.
pub(crate) fn encode_contents(s: &str) -> String {
    if !s.chars().any(|c| c == '^' || c.is_control()) {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '^' => out.push_str("^ "),
            '\t' => out.push_str("^I"),
            '\n' => out.push_str("^J"),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// The MTEXT's own tags: up to an embedded object (group 101), whose groups reuse MTEXT's codes.
fn own(tags: &[Tag]) -> &[Tag] {
    let end = tags.iter().position(|t| t.code == 101).unwrap_or(tags.len());
    tags.get(..end).unwrap_or(tags)
}

/// The tags before the column groups (where group 50 is the rotation angle).
pub(crate) fn before_columns(tags: &[Tag]) -> &[Tag] {
    let tags = own(tags);
    let end = tags.iter().position(|t| t.code == 75).unwrap_or(tags.len());
    tags.get(..end).unwrap_or(tags)
}

fn finite(v: f64) -> f64 {
    if v.is_finite() { v } else { 0.0 }
}

/// The columns of an MTEXT, and the column width (group 48) when there are columns.
pub(crate) fn read_columns(tags: &[Tag]) -> Option<(MTextColumns, f64)> {
    let tags = own(tags);
    let start = tags.iter().position(|t| t.code == 75)?;
    let kind = match tags.get(start).map(Tag::i64) {
        Some(1) => MTextColumnType::Static,
        Some(2) => MTextColumnType::Dynamic,
        _ => return None,
    };
    let cols = tags.get(start..).unwrap_or_default();
    let first = |code: i32| cols.iter().find(|t| t.code == code);
    let heights: Vec<f64> = cols.iter().filter(|t| t.code == 50).take(MAX_COLUMNS).map(|t| finite(t.f64()).max(0.0)).collect();
    let count = first(76).map(Tag::i64).unwrap_or(heights.len() as i64).clamp(1, MAX_COLUMNS as i64);
    let auto_height = first(79).map(Tag::i64).unwrap_or(0) != 0;
    let defined = tags.iter().find(|t| t.code == 46).map(|t| finite(t.f64()).max(0.0)).unwrap_or(0.0);
    let manual = kind == MTextColumnType::Dynamic && !auto_height;
    let height = if manual { heights.iter().copied().fold(0.0, f64::max) } else { heights.first().copied().filter(|h| *h > 0.0).unwrap_or(defined) };
    let c = MTextColumns {
        kind,
        count: u16::try_from(count).unwrap_or(1),
        gutter: first(49).map(|t| finite(t.f64()).max(0.0)).unwrap_or(0.0),
        height,
        auto_height,
        heights: if manual { heights } else { Vec::new() },
        flow_reversed: first(78).map(Tag::i64).unwrap_or(0) != 0,
    };
    Some((c, first(48).map(|t| finite(t.f64()).max(0.0)).unwrap_or(0.0)))
}

/// The column groups of an MTEXT with columns.
pub(crate) fn column_tags(t: &MText) -> Vec<Tag> {
    let Some(c) = &t.columns else { return Vec::new() };
    let manual = c.kind == MTextColumnType::Dynamic && !c.auto_height;
    let heights: Vec<f64> = if manual { c.heights.clone() } else { vec![c.height; usize::from(c.count.max(1))] };
    let count = if manual { heights.len().max(1) } else { usize::from(c.count.max(1)) };
    let mut v = vec![
        Tag::i(75, if c.kind == MTextColumnType::Dynamic { 2 } else { 1 }),
        Tag::i(76, count as i64),
        Tag::i(78, i64::from(c.flow_reversed)),
        Tag::i(79, i64::from(c.auto_height)),
        Tag::f(48, t.width),
        Tag::f(49, c.gutter),
    ];
    v.extend(heights.into_iter().map(|h| Tag::f(50, h)));
    v
}

#[cfg(test)]
mod chunk_tests {
    use super::*;

    #[test]
    fn a_caret_escape_is_never_split() {
        // carriage.dwg: a tab at a 250-character boundary came back from DWG as "^I".
        let text = format!("{}\t\tPGM", "x".repeat(249));
        let enc = encode_contents(&text);
        let parts = chunks(&enc);
        assert!(parts.iter().all(|p| !p.ends_with('^') && p.chars().count() <= 250), "{parts:?}");
        assert_eq!(parts.concat(), enc);
        assert!(parts.iter().all(|p| decode_contents(p).matches('^').count() == 0));
        assert!(chunks("").is_empty());
    }
}
