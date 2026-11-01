//! Number to text with ties rounded away from zero.
//!
//! Rust's `format!("{:.2}", 0.125)` rounds an exact tie to even (`0.12`), so a dimension of 2.5
//! at zero decimals would read `2`. Drafting practice (and AutoCAD's dimension text and `rtos`)
//! rounds a half up in magnitude, so a printed value never reads low on an exact half. The rule
//! used here works on the shortest decimal text that reads back as the same `f64`: 2.675 is
//! 2.67499999999999982236431605997495353221893310546875 in binary, but it is the number the user
//! typed, and it prints `2.68` at two places.

/// Digits of the shortest round-trip decimal of `a` (finite, `>= 0`) rounded half away from zero
/// to `prec` places: (integer digits, fraction digits of exactly `prec`).
fn round_digits(a: f64, prec: usize) -> (String, String) {
    let text = format!("{a}");
    let (int, frac) = text.split_once('.').unwrap_or((text.as_str(), ""));
    let mut digits: Vec<u8> = int.bytes().chain(frac.bytes().take(prec)).collect();
    digits.resize(int.len() + prec, b'0');
    if frac.as_bytes().get(prec).is_some_and(|d| *d >= b'5') {
        let mut i = digits.len();
        loop {
            if i == 0 {
                digits.insert(0, b'1');
                break;
            }
            i -= 1;
            match digits.get_mut(i) {
                Some(d) if *d == b'9' => *d = b'0',
                Some(d) => {
                    *d += 1;
                    break;
                }
                None => break,
            }
        }
    }
    let split = digits.len().saturating_sub(prec);
    let (i, f) = digits.split_at(split);
    (String::from_utf8_lossy(i).into_owned(), String::from_utf8_lossy(f).into_owned())
}

/// `v` with `prec` decimal places, ties away from zero (`2.5` at 0 places is `3`, `-0.125` at 2
/// is `-0.13`). A value that rounds to zero keeps its sign like `format!` (`-0.00`); non-finite
/// values print as `format!` does.
pub fn fixed(v: f64, prec: usize) -> String {
    if !v.is_finite() {
        return format!("{v:.prec$}");
    }
    // At 2^53 and above every f64 is a whole number: no tie to settle, and the exact binary
    // expansion is what `format!` prints.
    if v.abs() >= 9.0e15 {
        return format!("{v:.prec$}");
    }
    let (int, frac) = round_digits(v.abs(), prec);
    let sign = if v.is_sign_negative() { "-" } else { "" };
    if prec == 0 { format!("{sign}{int}") } else { format!("{sign}{int}.{frac}") }
}

/// `v` rounded to `prec` decimal places, ties away from zero, as a number.
pub fn round_to(v: f64, prec: usize) -> f64 {
    if !v.is_finite() {
        return v;
    }
    fixed(v, prec).parse().unwrap_or(v)
}

/// Scientific notation, `d.ddde<exp>` with `prec` mantissa decimals and ties away from zero
/// (Rust's `{:E}` form, without the exponent sign or padding rules of any one caller): the
/// mantissa text and the exponent.
pub fn sci_parts(v: f64, prec: usize) -> (String, i32) {
    if !v.is_finite() || v == 0.0 {
        return (fixed(if v.is_finite() { v } else { 0.0 }, prec), 0);
    }
    let text = format!("{:e}", v.abs());
    let (m, e) = text.split_once('e').unwrap_or((text.as_str(), "0"));
    let mut exp: i32 = e.parse().unwrap_or(0);
    let mut mant = fixed(m.parse().unwrap_or(1.0), prec);
    if mant.starts_with("10") {
        exp += 1;
        mant = fixed(1.0, prec);
    }
    if v < 0.0 {
        mant.insert(0, '-');
    }
    (mant, exp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn halves_round_away_from_zero() {
        for (v, want) in [(0.5, "1"), (1.5, "2"), (2.5, "3"), (3.5, "4"), (4.5, "5")] {
            assert_eq!(fixed(v, 0), want, "{v}");
        }
        assert_eq!(fixed(0.125, 2), "0.13");
        assert_eq!(fixed(0.375, 2), "0.38");
        assert_eq!(fixed(-2.5, 0), "-3");
        assert_eq!(fixed(-0.125, 2), "-0.13");
    }

    #[test]
    fn binary_ties_follow_the_decimal_text() {
        assert_eq!(fixed(2.675, 2), "2.68");
        assert_eq!(fixed(1.005, 2), "1.01");
        assert_eq!(fixed(2.674, 2), "2.67");
        assert_eq!(fixed(1.0049999, 2), "1.00");
    }

    #[test]
    fn carries_and_padding() {
        assert_eq!(fixed(9.995, 2), "10.00");
        assert_eq!(fixed(99.5, 0), "100");
        assert_eq!(fixed(0.96, 1), "1.0");
        assert_eq!(fixed(3.0, 3), "3.000");
        assert_eq!(fixed(0.0, 2), "0.00");
        assert_eq!(fixed(12.0, 0), "12");
        assert_eq!(fixed(1e21, 1), format!("{:.1}", 1e21));
    }

    #[test]
    fn unrounded_values_match_format() {
        for v in [0.1, 1.0 / 3.0, 123.456, -7.26, 1e-9, 0.0004999] {
            for p in 0..=8usize {
                let exact = format!("{v:.p$}");
                let ours = fixed(v, p);
                // Only an exact tie differs, and none of these is one.
                assert_eq!(ours, exact, "{v} at {p}");
            }
        }
    }

    #[test]
    fn rounds_to_a_number() {
        assert_eq!(round_to(2.675, 2), 2.68);
        assert_eq!(round_to(-2.5, 0), -3.0);
        assert_eq!(round_to(f64::INFINITY, 2), f64::INFINITY);
    }

    #[test]
    fn scientific_ties_and_carry() {
        assert_eq!(sci_parts(1.25, 1), ("1.3".to_string(), 0));
        assert_eq!(sci_parts(9.95, 1), ("1.0".to_string(), 1));
        assert_eq!(sci_parts(-12500.0, 1), ("-1.3".to_string(), 4));
        assert_eq!(sci_parts(0.0, 2), ("0.00".to_string(), 0));
    }
}
