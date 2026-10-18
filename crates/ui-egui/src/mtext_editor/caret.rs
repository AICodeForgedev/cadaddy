//! Where the editor's caret positions are on the laid-out MTEXT: the editor's plain text (one
//! character per character, `\n` between paragraphs) matched to the lines of
//! [`cadaddy_fonts::MTextLayout::carets`], the final object's own layout. Hit-testing, Up/Down,
//! Home/End and the selection use this map, so they follow the text as it will be drawn.

use cadaddy_fonts::CaretLine;
use cadaddy_geom::Vec2;

/// Every caret position (0..=chars) placed on a laid-out line.
#[derive(Clone, Debug, Default)]
pub(super) struct CaretMap {
    lines: Vec<CaretLine>,
    /// For caret position `i` (before character `i`): its line and x.
    place: Vec<(usize, f64)>,
}

/// Whether the editor's character `e` is the line's character `l` (a tab and a non-breaking space
/// are laid out as spaces).
fn same(e: char, l: char) -> bool {
    e == l || (l == ' ' && (e == '\t' || e == '\u{a0}'))
}

impl CaretMap {
    /// Place the caret positions of `text` on `lines`.
    pub(super) fn new(text: &str, lines: Vec<CaretLine>) -> Self {
        let chars: Vec<char> = text.chars().collect();
        let lchars: Vec<Vec<char>> = lines.iter().map(|l| l.text.chars().collect()).collect();
        let x_at = |li: usize, ci: usize| -> f64 {
            match lines.get(li) {
                Some(l) => l.xs.get(ci).copied().unwrap_or(l.end),
                None => 0.0,
            }
        };
        let mut place = Vec::with_capacity(chars.len() + 1);
        let (mut li, mut ci) = (0usize, 0usize);
        for &c in &chars {
            loop {
                let Some(lt) = lchars.get(li) else {
                    // Past the last line: everything at its end.
                    let last = lines.len().saturating_sub(1);
                    place.push((last, x_at(last, usize::MAX)));
                    break;
                };
                if lt.get(ci).is_some_and(|l| same(c, *l)) {
                    place.push((li, x_at(li, ci)));
                    ci += 1;
                    break;
                }
                let para = lines.get(li).map_or(0, |l| l.para);
                if c == '\n' {
                    // A paragraph break: the caret before it is at the end of this line; the next
                    // paragraph starts on its first line.
                    place.push((li, x_at(li, usize::MAX)));
                    let mut next = li + 1;
                    while lines.get(next).is_some_and(|l| l.para == para) {
                        next += 1;
                    }
                    (li, ci) = (next, 0);
                    break;
                }
                let wrapped_on = ci >= lt.len() && lines.get(li + 1).is_some_and(|l| l.para == para);
                if wrapped_on {
                    if c == ' ' || c == '\t' {
                        // The space where the line wrapped: at the end of this line.
                        place.push((li, x_at(li, usize::MAX)));
                        (li, ci) = (li + 1, 0);
                        break;
                    }
                    (li, ci) = (li + 1, 0);
                    continue;
                }
                // Not laid out (a leading or repeated space): where the caret already is.
                place.push((li.min(lines.len().saturating_sub(1)), x_at(li, ci)));
                break;
            }
        }
        let end = match lines.get(li) {
            Some(_) => (li, x_at(li, ci)),
            None => {
                let last = lines.len().saturating_sub(1);
                (last, x_at(last, usize::MAX))
            }
        };
        place.push(end);
        CaretMap { lines, place }
    }

    /// The number of caret positions (characters + 1).
    pub(super) fn len(&self) -> usize {
        self.place.len()
    }

    /// The line of caret position `i` and its x, baseline and height.
    pub(super) fn pos(&self, i: usize) -> Option<(f64, f64, f64)> {
        let &(li, x) = self.place.get(i).or(self.place.last())?;
        let l = self.lines.get(li)?;
        Some((x, l.baseline, l.height))
    }

    fn line_of(&self, i: usize) -> Option<usize> {
        self.place.get(i).or(self.place.last()).map(|p| p.0)
    }

    /// The caret positions on line `li`, with their x.
    fn on_line(&self, li: usize) -> impl Iterator<Item = (usize, f64)> + '_ {
        self.place.iter().enumerate().filter(move |(_, p)| p.0 == li).map(|(i, p)| (i, p.1))
    }

    /// The caret position on line `li` nearest `x` (the first of equally near ones).
    fn nearest_on(&self, li: usize, x: f64) -> Option<usize> {
        self.on_line(li).min_by(|a, b| (a.1 - x).abs().total_cmp(&(b.1 - x).abs()).then(a.0.cmp(&b.0))).map(|p| p.0)
    }

    /// The caret position under point `p` (text frame): the nearest line, then the nearest
    /// character boundary on it.
    pub(super) fn hit(&self, p: Vec2) -> usize {
        let dist = |l: &CaretLine| {
            let (top, bottom) = (l.baseline + l.height, l.baseline - l.height * 0.4);
            let dy = if p.y > top {
                p.y - top
            } else if p.y < bottom {
                bottom - p.y
            } else {
                0.0
            };
            let x0 = l.xs.first().copied().unwrap_or(l.end).min(l.end);
            let dx = if p.x < x0 - l.height {
                x0 - l.height - p.x
            } else if p.x > l.end + l.height {
                p.x - l.end - l.height
            } else {
                0.0
            };
            // The nearest line up or down; lines side by side (columns) by how far across.
            dy + dx * 1e-3
        };
        let best = self.lines.iter().enumerate().min_by(|a, b| dist(a.1).total_cmp(&dist(b.1))).map(|(i, _)| i);
        best.and_then(|li| self.nearest_on(li, p.x)).unwrap_or(self.len().saturating_sub(1))
    }

    /// Up (`dir` -1) or Down (+1) from caret position `i`, toward x `keep_x`.
    pub(super) fn vertical(&self, i: usize, dir: i32, keep_x: f64) -> usize {
        let Some(li) = self.line_of(i) else { return i };
        let target = if dir < 0 { li.checked_sub(1) } else { Some(li + 1).filter(|t| *t < self.lines.len()) };
        match target {
            Some(t) => self.nearest_on(t, keep_x).unwrap_or(i),
            // Up from the first line: its start; Down from the last: its end.
            None if dir < 0 => self.line_start(i),
            None => self.line_end(i),
        }
    }

    /// The first caret position on caret position `i`'s line (Home).
    pub(super) fn line_start(&self, i: usize) -> usize {
        self.line_of(i).and_then(|li| self.on_line(li).map(|p| p.0).min()).unwrap_or(0)
    }

    /// The last caret position on caret position `i`'s line (End).
    pub(super) fn line_end(&self, i: usize) -> usize {
        self.line_of(i).and_then(|li| self.on_line(li).map(|p| p.0).max()).unwrap_or(0)
    }

    /// The selected characters `a..b` as spans on their lines: (x0, x1, baseline, height).
    pub(super) fn spans(&self, a: usize, b: usize) -> Vec<(f64, f64, f64, f64)> {
        let mut out: Vec<(usize, f64, f64)> = Vec::new();
        for i in a.min(b)..a.max(b) {
            let (Some(&(li, x)), next) = (self.place.get(i), self.place.get(i + 1)) else { continue };
            let x1 = match next {
                Some(&(nl, nx)) if nl == li => nx,
                _ => self.lines.get(li).map_or(x, |l| l.end),
            };
            match out.last_mut() {
                Some(s) if s.0 == li => {
                    s.1 = s.1.min(x.min(x1));
                    s.2 = s.2.max(x.max(x1));
                }
                _ => out.push((li, x.min(x1), x.max(x1))),
            }
        }
        out.into_iter().filter_map(|(li, x0, x1)| self.lines.get(li).map(|l| (x0, x1, l.baseline, l.height))).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str, para: usize, x0: f64, baseline: f64) -> CaretLine {
        // Every character one unit wide.
        let n = text.chars().count();
        CaretLine { text: text.into(), para, xs: (0..n).map(|i| x0 + i as f64).collect(), end: x0 + n as f64, baseline, height: 1.0 }
    }

    #[test]
    fn wrapped_text_and_paragraphs_map_to_their_lines() {
        // "abc def" wrapped after "abc"; then a second paragraph "gh".
        let m = CaretMap::new("abc def\ngh", vec![line("abc", 0, 0.0, -1.0), line("def", 0, 0.0, -2.7), line("gh", 1, 0.0, -4.4)]);
        assert_eq!(m.len(), 11);
        let at = |i| m.pos(i).map(|(x, y, _)| (x, y));
        assert_eq!(at(0), Some((0.0, -1.0)));
        assert_eq!(at(3), Some((3.0, -1.0)), "the wrap space: end of the first line");
        assert_eq!(at(4), Some((0.0, -2.7)), "d starts the second line");
        assert_eq!(at(7), Some((3.0, -2.7)), "before the paragraph break");
        assert_eq!(at(8), Some((0.0, -4.4)));
        assert_eq!(at(10), Some((2.0, -4.4)), "the end");
    }

    #[test]
    fn hit_testing_home_end_and_vertical_moves_follow_the_lines() {
        let m = CaretMap::new("abc def\ngh", vec![line("abc", 0, 0.0, -1.0), line("def", 0, 0.0, -2.7), line("gh", 1, 0.0, -4.4)]);
        assert_eq!(m.hit(Vec2::new(1.4, -0.5)), 1);
        assert_eq!(m.hit(Vec2::new(1.6, -2.5)), 6);
        assert_eq!(m.hit(Vec2::new(50.0, -4.3)), 10, "far right of the last line: its end");
        assert_eq!(m.hit(Vec2::new(-3.0, -2.6)), 4, "left of the second line: its start");
        assert_eq!(m.line_start(5), 4);
        assert_eq!(m.line_end(5), 7);
        assert_eq!(m.line_end(1), 3);
        assert_eq!(m.vertical(1, 1, 1.0), 5, "down keeps the x");
        assert_eq!(m.vertical(5, 1, 1.0), 9);
        assert_eq!(m.vertical(9, 1, 1.0), 10, "down from the last line: its end");
        assert_eq!(m.vertical(9, -1, 1.0), 5);
        assert_eq!(m.vertical(1, -1, 1.0), 0, "up from the first line: its start");
    }

    #[test]
    fn tabs_indents_and_unlaid_spaces() {
        // "\ta" on a line laid out as " a" with the tab ending at 5; a leading space not laid out.
        let mut l = line(" a", 0, 0.0, -1.0);
        l.xs = vec![0.0, 5.0];
        l.end = 6.0;
        let m = CaretMap::new("\ta", vec![l.clone()]);
        assert_eq!(m.pos(1).map(|p| p.0), Some(5.0));
        // A leading space the layout leaves out: the caret stays at the line's start.
        let m = CaretMap::new(" ab", vec![line("ab", 0, 0.0, -1.0)]);
        assert_eq!(m.pos(0).map(|p| p.0), Some(0.0));
        assert_eq!(m.pos(1).map(|p| p.0), Some(0.0), "the unlaid space doesn't move the caret");
        assert_eq!(m.pos(2).map(|p| p.0), Some(1.0));
        assert_eq!(m.pos(3).map(|p| p.0), Some(2.0));
        drop(l);
    }

    #[test]
    fn selection_spans_per_line() {
        let m = CaretMap::new("abc def", vec![line("abc", 0, 0.0, -1.0), line("def", 0, 0.0, -2.7)]);
        let s = m.spans(1, 6);
        assert_eq!(s.len(), 2);
        assert_eq!((s[0].0, s[0].1), (1.0, 3.0));
        assert_eq!((s[1].0, s[1].1), (0.0, 2.0));
        assert!(m.spans(2, 2).is_empty());
    }

    #[test]
    fn real_layout_tab_stops_and_wraps() {
        // The fonts crate's layout of "a<tab>b" with a tab stop at 5: b's caret position at 5.
        let lay = cadaddy_fonts::layout_mtext("\\pxt5;a\tb", Vec2::ZERO, 1.0, 20.0, 1, 0.0, 1.0);
        let m = CaretMap::new("a\tb", lay.carets);
        assert!(m.pos(2).is_some_and(|p| (p.0 - 5.0).abs() < 1e-9), "{:?}", m.pos(2));
        // Wrapped: the second word starts the second line, below the first.
        let w = cadaddy_fonts::line_width("alpha", 1.0, 1.0) + 0.1;
        let lay = cadaddy_fonts::layout_mtext("alpha beta", Vec2::ZERO, 1.0, w, 1, 0.0, 1.0);
        let m = CaretMap::new("alpha beta", lay.carets);
        let (a, b) = (m.pos(0).unwrap_or_default(), m.pos(6).unwrap_or_default());
        assert!(b.0.abs() < 1e-9 && b.1 < a.1, "{a:?} {b:?}");
        assert_eq!(m.vertical(1, 1, m.pos(1).map_or(0.0, |p| p.0)), 7);
    }

    #[test]
    fn empty_and_hostile_inputs() {
        let m = CaretMap::new("", vec![line("", 0, 0.0, -1.0)]);
        assert_eq!(m.len(), 1);
        assert_eq!(m.pos(0).map(|p| p.0), Some(0.0));
        let m = CaretMap::new("abc", Vec::new());
        assert_eq!(m.len(), 4);
        assert!(m.pos(2).is_none(), "no lines: no place");
        assert_eq!(m.hit(Vec2::new(f64::NAN, 0.0)), 3);
        let _ = m.vertical(99, 1, 0.0);
        assert!(m.spans(0, 99).is_empty());
    }
}
