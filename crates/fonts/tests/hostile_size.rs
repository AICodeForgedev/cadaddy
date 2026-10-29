//! Hostile-size MTEXT: one very long run (here 20,000 groups of escaped-backslash text, as a
//! battle-test file has it) must lay out in linear time. The carets of a long run were measured
//! one prefix at a time, which took minutes.

use cadaddy_fonts::layout_mtext;
use cadaddy_geom::Vec2;
use std::time::Instant;

fn lay(s: &str) -> cadaddy_fonts::MTextLayout {
    layout_mtext(s, Vec2::ZERO, 1.0, 0.0, 1, 0.0, 1.0)
}

#[test]
fn nested_groups_of_long_text_lay_out_fast() {
    let n = 20_000;
    let mut s = "{\\\\H2;".repeat(n);
    s.push_str(&"}".repeat(n));
    let t = Instant::now();
    let l = lay(&s);
    // Seconds before the fix: 300; now well under one. 20 s leaves room for a loaded runner.
    assert!(t.elapsed().as_secs() < 20, "layout took {:?}", t.elapsed());
    let caret = l.carets.first().expect("one line");
    assert_eq!(caret.xs.len(), caret.text.chars().count());
    assert!(caret.xs.windows(2).all(|w| w[0] <= w[1] + 1e-9), "caret starts must not go backwards");
    assert!(caret.xs.last().is_some_and(|x| *x <= caret.end + 1e-9));
}

#[test]
fn long_run_carets_follow_the_true_advance() {
    // "ab" repeated: one pair advances by width("abab") - width("ab"), whatever the run's length.
    let font = cadaddy_fonts::TextFont::Stroke;
    let pair = cadaddy_fonts::text_width(&font, "abab", 1.0, 1.0) - cadaddy_fonts::text_width(&font, "ab", 1.0, 1.0);
    let long = lay(&"ab".repeat(1_500));
    let xs = &long.carets[0].xs;
    assert_eq!(xs.len(), 3_000);
    assert!((xs[2_998] - 1_499.0 * pair).abs() < 1e-2 * 1_499.0 * pair, "{} vs {}", xs[2_998], 1_499.0 * pair);
    assert!(xs.windows(2).all(|w| w[0] < w[1]));
}

#[test]
fn many_formatted_pieces_stay_linear() {
    // 30,000 one-character pieces in alternating formats on one line.
    let s = "{\\H2;a}{\\H3;b}".repeat(15_000);
    let t = Instant::now();
    let l = lay(&s);
    assert!(t.elapsed().as_secs() < 20, "layout took {:?}", t.elapsed());
    assert!(!l.carets.is_empty());
}
