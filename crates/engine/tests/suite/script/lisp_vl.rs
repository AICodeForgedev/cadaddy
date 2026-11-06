//! The Visual LISP utilities: strings, lists, sorting, functions of functions, file names and
//! read-only file information, and catching errors. Each with its bad arguments.

use cadaddy_engine::Session;
use serde_json::{Value, json};

/// The value of `code` as `prin1` shows it, or the error message.
fn eval(s: &mut Session, code: &str) -> Result<String, String> {
    let r: Value = s.execute("lisp", &json!({ "code": code })).map_err(|e| e.to_string())?;
    match r.get("error").and_then(Value::as_str) {
        Some(e) => Err(e.to_string()),
        None => Ok(r["value"].as_str().unwrap_or_default().to_string()),
    }
}

fn ok(s: &mut Session, code: &str) -> String {
    eval(s, code).unwrap_or_else(|e| panic!("{code}: {e}"))
}

/// `code` fails with a message containing `want`.
fn fails(s: &mut Session, code: &str, want: &str) {
    match eval(s, code) {
        Ok(v) => panic!("{code} gave {v}, expected an error"),
        Err(e) => assert!(e.contains(want), "{code}: {e} (wanted {want})"),
    }
}

#[test]
fn string_functions() {
    let mut s = Session::new();
    assert_eq!(ok(&mut s, r#"(vl-string-search "lo" "hello hello")"#), "3");
    assert_eq!(ok(&mut s, r#"(vl-string-search "lo" "hello hello" 4)"#), "9");
    assert_eq!(ok(&mut s, r#"(vl-string-search "x" "hello")"#), "nil");
    fails(&mut s, r#"(vl-string-search 1 "x")"#, "stringp");

    assert_eq!(ok(&mut s, r#"(vl-string-subst "Ex" "ex" "ex ex")"#), "\"Ex ex\"", "the first only");
    fails(&mut s, r#"(vl-string-subst "a" "b")"#, "too few arguments");

    assert_eq!(ok(&mut s, r#"(vl-string-trim " \t" "  pad  ")"#), "\"pad\"");
    assert_eq!(ok(&mut s, r#"(vl-string-left-trim " " "  pad  ")"#), "\"pad  \"");
    assert_eq!(ok(&mut s, r#"(vl-string-right-trim " " "  pad  ")"#), "\"  pad\"");
    fails(&mut s, r#"(vl-string-trim nil "x")"#, "stringp");

    assert_eq!(ok(&mut s, r#"(vl-string-translate "abc" "ABC" "a cab")"#), "\"A CAB\"");
    fails(&mut s, r#"(vl-string-translate "a" "b")"#, "too few arguments");

    // vl-string-position: from the start, from a position, backward from the end.
    assert_eq!(ok(&mut s, r#"(vl-string-position 97 "banana")"#), "1");
    assert_eq!(ok(&mut s, r#"(vl-string-position 97 "banana" 2)"#), "3");
    assert_eq!(ok(&mut s, r#"(vl-string-position 97 "banana" 0 T)"#), "5");
    assert_eq!(ok(&mut s, r#"(vl-string-position 97 "banana" 2 T)"#), "5");
    assert_eq!(ok(&mut s, r#"(vl-string-position 122 "banana")"#), "nil");
    assert_eq!(ok(&mut s, r#"(vl-string-position 97 "banana" 99)"#), "nil");
    fails(&mut s, r#"(vl-string-position "a" "banana")"#, "fixnump");
    fails(&mut s, r#"(vl-string-position 97 "banana" -1)"#, "fixnump");

    assert_eq!(ok(&mut s, r#"(vl-string-mismatch "VL-FUN" "VL-VAR")"#), "3");
    assert_eq!(ok(&mut s, r#"(vl-string-mismatch "vl-fun" "VL-VAR" 0 0 T)"#), "3");
    assert_eq!(ok(&mut s, r#"(vl-string-mismatch "abc" "xbc" 1 1)"#), "2");
    fails(&mut s, r#"(vl-string-mismatch "a" 1)"#, "stringp");

    assert_eq!(ok(&mut s, r#"(vl-string-elt "ABC" 1)"#), "66");
    fails(&mut s, r#"(vl-string-elt "ABC" 3)"#, "out of range");
    fails(&mut s, r#"(vl-string-elt "ABC" 1.0)"#, "fixnump");

    assert_eq!(ok(&mut s, r#"(vl-string->list "AB")"#), "(65 66)");
    assert_eq!(ok(&mut s, "(vl-list->string '(72 105))"), "\"Hi\"");
    fails(&mut s, r#"(vl-string->list 5)"#, "stringp");
}

#[test]
fn list_functions() {
    let mut s = Session::new();
    assert_eq!(ok(&mut s, "(vl-position 'c '(a b c))"), "2");
    assert_eq!(ok(&mut s, "(vl-position 'z '(a b c))"), "nil");
    assert_eq!(ok(&mut s, "(vl-remove 2 '(1 2 3 2))"), "(1 3)");
    assert_eq!(ok(&mut s, "(vl-consp '(1))"), "T");
    assert_eq!(ok(&mut s, "(vl-consp nil)"), "nil");
    assert_eq!(ok(&mut s, "(vl-list* 1 2 '(3 4))"), "(1 2 3 4)");
    assert_eq!(ok(&mut s, "(vl-list* 1 2)"), "(1 . 2)");
    assert_eq!(ok(&mut s, "(vl-list* 1)"), "1");
    fails(&mut s, "(vl-list*)", "too few arguments");
    assert_eq!(ok(&mut s, "(vl-symbol-name 'abc)"), "\"ABC\"");
    fails(&mut s, "(vl-symbol-name 1)", "symbolp");
    assert_eq!(ok(&mut s, r#"(vl-princ-to-string "a b")"#), "\"a b\"");
    assert_eq!(ok(&mut s, r#"(vl-prin1-to-string "a b")"#), "\"\\\"a b\\\"\"");
}

#[test]
fn functions_of_functions() {
    let mut s = Session::new();
    assert_eq!(ok(&mut s, "(vl-sort '(3 1 2) '<)"), "(1 2 3)");
    assert_eq!(ok(&mut s, r#"(vl-sort '("b" "a") (function (lambda (x y) (< x y))))"#), "(\"a\" \"b\")");
    assert_eq!(ok(&mut s, "(vl-sort-i '(3 1 2) '<)"), "(1 2 0)");
    assert_eq!(ok(&mut s, "(vl-sort-i '(3 1 3 1) '>)"), "(0 2 1 3)", "stable: equal ones keep their order");
    assert_eq!(ok(&mut s, "(vl-sort-i nil '<)"), "nil");
    fails(&mut s, "(vl-sort-i '(1 2) 'nofunc)", "no function definition");
    fails(&mut s, "(vl-sort-i 5 '<)", "listp");
    fails(&mut s, r#"(vl-sort-i '(1 "a") '<)"#, "bad argument type");

    assert_eq!(ok(&mut s, "(vl-remove-if 'minusp '(-1 2 -3))"), "(2)");
    assert_eq!(ok(&mut s, "(vl-remove-if-not 'minusp '(-1 2 -3))"), "(-1 -3)");
    assert_eq!(ok(&mut s, "(vl-member-if 'minusp '(1 -2 3))"), "(-2 3)");
    assert_eq!(ok(&mut s, "(vl-member-if-not 'numberp '(1 2 a 3))"), "(A 3)");
    assert_eq!(ok(&mut s, "(vl-member-if-not 'numberp '(1 2))"), "nil");
    fails(&mut s, "(vl-member-if-not 'numberp 5)", "listp");
    assert_eq!(ok(&mut s, "(vl-some '= '(1 2) '(3 2))"), "T");
    assert_eq!(ok(&mut s, "(vl-every 'numberp '(1 2))"), "T");
    assert_eq!(ok(&mut s, "(vl-every 'numberp '(1 a))"), "nil");
    fails(&mut s, "(vl-every 'numberp)", "too few arguments");
}

#[test]
fn file_names() {
    let mut s = Session::new();
    assert_eq!(ok(&mut s, r#"(vl-filename-base "c:/plans/site.dwg")"#), "\"site\"");
    assert_eq!(ok(&mut s, r#"(vl-filename-extension "c:/plans/site.dwg")"#), "\".dwg\"");
    assert_eq!(ok(&mut s, r#"(vl-filename-extension "c:/plans/site")"#), "nil");
    assert_eq!(ok(&mut s, r#"(vl-filename-directory "c:/plans/site.dwg")"#), "\"c:/plans\"");
    fails(&mut s, "(vl-filename-base 1)", "stringp");
}

#[test]
fn file_information_is_read_only() {
    let mut s = Session::new();
    let dir = std::env::temp_dir().join(format!("cadaddy-lisp-vl-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("a.lsp"), b"(princ)").unwrap();
    std::fs::write(dir.join("b.dwg"), b"0123456789").unwrap();
    let d = dir.to_string_lossy().replace('\\', "/");
    let q = |p: &str| format!("\"{d}/{p}\"");

    assert_eq!(ok(&mut s, &format!("(vl-file-size {})", q("b.dwg"))), "10");
    assert_eq!(ok(&mut s, &format!("(vl-file-size {})", q("sub"))), "0");
    assert_eq!(ok(&mut s, &format!("(vl-file-size {})", q("none"))), "nil");
    fails(&mut s, "(vl-file-size 1)", "stringp");

    assert_eq!(ok(&mut s, &format!("(vl-file-directory-p {})", q("sub"))), "T");
    assert_eq!(ok(&mut s, &format!("(vl-file-directory-p {})", q("a.lsp"))), "nil");

    // (year month day-of-week day hours minutes seconds ms), a plausible date.
    let t = ok(&mut s, &format!("(vl-file-systime {})", q("a.lsp")));
    let parts: Vec<i64> = t.trim_matches(['(', ')']).split(' ').map(|x| x.parse().unwrap()).collect();
    assert_eq!(parts.len(), 8, "{t}");
    assert!(parts[0] >= 2024 && (1..=12).contains(&parts[1]) && (0..=6).contains(&parts[2]) && (1..=31).contains(&parts[3]), "{t}");
    assert_eq!(ok(&mut s, &format!("(vl-file-systime {})", q("sub"))), "nil");

    let dq = format!("\"{d}\"");
    assert_eq!(ok(&mut s, &format!("(vl-directory-files {dq} \"*.*\" 1)")), "(\"a.lsp\" \"b.dwg\")");
    assert_eq!(ok(&mut s, &format!("(vl-directory-files {dq} \"*.lsp\")")), "(\"a.lsp\")");
    assert_eq!(ok(&mut s, &format!("(vl-directory-files {dq} nil -1)")), "(\"sub\")");
    assert_eq!(ok(&mut s, &format!("(vl-directory-files {dq} nil 0)")), "(\"a.lsp\" \"b.dwg\" \"sub\")");
    assert_eq!(ok(&mut s, &format!("(vl-directory-files {})", q("missing"))), "nil");
    fails(&mut s, "(vl-directory-files 1)", "stringp");
    fails(&mut s, &format!("(vl-directory-files {dq} \"*\" \"x\")"), "fixnump");
    // Nothing was created or changed.
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 3);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn catching_errors() {
    let mut s = Session::new();
    assert_eq!(ok(&mut s, "(vl-catch-all-apply '+ '(1 2))"), "3");
    assert_eq!(ok(&mut s, "(vl-catch-all-error-p (vl-catch-all-apply '/ '(1 0)))"), "T");
    assert_eq!(ok(&mut s, "(vl-catch-all-error-message (vl-catch-all-apply '/ '(1 0)))"), "\"divide by zero\"");
    assert_eq!(ok(&mut s, "(vl-catch-all-error-p 5)"), "nil");
    fails(&mut s, "(vl-catch-all-error-message 5)", "bad argument type");
    fails(&mut s, "(vl-catch-all-apply '+ 5)", "listp");
}
