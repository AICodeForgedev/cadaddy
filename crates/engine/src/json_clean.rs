//! Strip control characters from JSON values so every serialised output is accepted by a strict
//! JSON parser.  `serde_json` already escapes control chars in `Value::String` during
//! serialisation, but downstream tools that concatenate multiple outputs (CLI stdout + stderr,
//! log lines) or parse with non-standard decoders can trip on control characters that survive
//! in the Rust `String` values inside a `Value` tree.  This module provides a cheap pass that
//! replaces every C0 control character (U+0000–U+001F) except the three that JSON allows
//! inside strings when escaped (`\t`, `\n`, `\r`) with a Unicode replacement.

use serde_json::Value;

/// Replace C0 control characters in every string inside `v` in place.
/// Tabs, newlines and carriage returns are kept; everything else in 0x00–0x1F is replaced with
/// the Unicode replacement character U+FFFD.
pub fn sanitize(v: &mut Value) {
    match v {
        Value::String(s) => sanitize_string_in_place(s),
        Value::Array(a) => a.iter_mut().for_each(sanitize),
        Value::Object(m) => m.values_mut().for_each(sanitize),
        _ => {}
    }
}

/// Return a sanitised clone (leaves the original untouched).
pub fn sanitized(v: Value) -> Value {
    let mut v = v;
    sanitize(&mut v);
    v
}

/// In-place replacement on a single string: only allocates when there is something to replace.
pub fn sanitize_string_in_place(s: &mut String) {
    if !s.bytes().any(|b| b < 0x20 && b != b'\t' && b != b'\n' && b != b'\r') {
        return;
    }
    *s = s.chars().map(|c| if (c as u32) < 0x20 && c != '\t' && c != '\n' && c != '\r' { '\u{FFFD}' } else { c }).collect();
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn clean_string_is_untouched() {
        let mut v = json!("hello world");
        sanitize(&mut v);
        assert_eq!(v, json!("hello world"));
    }

    #[test]
    fn tabs_newlines_carriage_returns_are_kept() {
        let mut v = json!("a\tb\nc\r");
        sanitize(&mut v);
        assert_eq!(v, json!("a\tb\nc\r"));
    }

    #[test]
    fn null_and_escape_are_replaced() {
        let mut v = json!(format!("before\x00mid\x1bafter"));
        sanitize(&mut v);
        assert_eq!(v.as_str().unwrap(), "before\u{FFFD}mid\u{FFFD}after");
    }

    #[test]
    fn nested_values_are_cleaned() {
        let mut v = json!({"a": [format!("x\x01y")], "b": {"c": format!("z\x02w")}});
        sanitize(&mut v);
        assert_eq!(v["a"][0].as_str().unwrap(), "x\u{FFFD}y");
        assert_eq!(v["b"]["c"].as_str().unwrap(), "z\u{FFFD}w");
    }

    #[test]
    fn numbers_and_booleans_are_untouched() {
        let mut v = json!({"n": 42, "b": true, "x": null});
        let before = v.clone();
        sanitize(&mut v);
        assert_eq!(v, before);
    }

    #[test]
    fn serialised_output_is_strict_json() {
        let raw = format!("esc\x1b[31mred\x1b[0m bell\x07 null\x00");
        let mut v = json!({"text": raw});
        sanitize(&mut v);
        let s = serde_json::to_string(&v).unwrap();
        // A strict JSON parser must accept it: no raw control characters anywhere.
        assert!(!s.bytes().any(|b| b < 0x20 && b != b'\t' && b != b'\n' && b != b'\r'), "raw control chars in serialised JSON: {s:?}");
        // And a round-trip parse succeeds.
        let _: Value = serde_json::from_str(&s).unwrap();
    }
}
