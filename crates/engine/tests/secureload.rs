//! SECURELOAD and TRUSTEDPATHS: LISP and scripts load silently only from trusted folders;
//! elsewhere SECURELOAD 1 asks (the refusal keeps the question), 2 refuses, 0 loads anything. Code
//! can't loosen the rules or write code files.

use std::path::{Path, PathBuf};

use cadaddy_engine::Session;
use serde_json::json;

fn folder(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-secureload-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("sub")).unwrap();
    d
}

fn lsp(dir: &Path, name: &str) -> String {
    let p = dir.join(name);
    std::fs::write(&p, "(defun c:hello () (princ \"hi\")) (setq loaded-marker 42)").unwrap();
    p.to_string_lossy().into_owned()
}

fn marker(s: &mut Session) -> String {
    s.execute("lisp", &json!({ "code": "loaded-marker" })).unwrap()["value"].as_str().unwrap_or_default().to_string()
}

#[test]
fn untrusted_files_ask_at_1_refuse_at_2_and_load_at_0() {
    let dir = folder("levels");
    let file = lsp(&dir, "tool.lsp");
    let mut s = Session::new();
    assert_eq!(s.settings.secureload, 1, "the default asks");
    let e = s.execute("appload", &json!({ "path": file })).unwrap_err().to_string();
    assert!(e.contains("isn't in a trusted folder"), "{e}");
    assert_eq!(marker(&mut s), "nil", "nothing ran");
    let ask = s.secureload_ask.clone().expect("the question is kept for the app");
    assert_eq!(ask.retry.as_ref().map(|r| r.0.as_str()), Some("appload"));
    // Allowed once: it loads, and stays allowed this session.
    s.execute("secureload.allow", &json!({ "path": file })).unwrap();
    assert!(s.secureload_ask.is_none());
    s.execute("appload", &json!({ "path": file })).unwrap();
    assert_eq!(marker(&mut s), "42");

    let mut s = Session::new();
    s.execute("setvar", &json!({ "name": "SECURELOAD", "value": 2 })).unwrap();
    assert!(s.execute("appload", &json!({ "path": file })).unwrap_err().to_string().contains("SECURELOAD is 2"));
    assert!(s.secureload_ask.is_none(), "2 asks nothing");

    let mut s = Session::new();
    s.execute("setvar", &json!({ "name": "SECURELOAD", "value": 0 })).unwrap();
    s.execute("appload", &json!({ "path": file })).unwrap();
    assert_eq!(marker(&mut s), "42");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn trusted_paths_and_the_support_folder_load_silently() {
    let dir = folder("trusted");
    let top = lsp(&dir, "top.lsp");
    let deep = lsp(&dir.join("sub"), "deep.lsp");
    let mut s = Session::new();
    s.execute("setvar", &json!({ "name": "TRUSTEDPATHS", "value": dir.to_string_lossy() })).unwrap();
    s.execute("appload", &json!({ "path": top })).unwrap();
    assert!(s.execute("appload", &json!({ "path": deep })).is_err(), "a folder alone doesn't include its subfolders");
    let with_sub = format!("{}{}...", dir.to_string_lossy(), std::path::MAIN_SEPARATOR);
    s.execute("setvar", &json!({ "name": "TRUSTEDPATHS", "value": format!("/somewhere/else;{with_sub}") })).unwrap();
    s.execute("appload", &json!({ "path": deep })).unwrap();
    // Always: the folder joins TRUSTEDPATHS.
    let other = folder("always");
    let f = lsp(&other, "a.lsp");
    s.execute("secureload.allow", &json!({ "path": f, "always": true })).unwrap();
    assert!(s.settings.trustedpaths.contains(other.file_name().unwrap().to_str().unwrap()), "{}", s.settings.trustedpaths);
    s.execute("appload", &json!({ "path": lsp(&other, "b.lsp") })).unwrap();
    // The support folder is always trusted.
    let support = folder("support");
    let mut s = Session::new();
    s.set_lisp_support_dir(Some(support.clone()));
    s.execute("appload", &json!({ "path": lsp(&support, "mine.lsp") })).unwrap();
    for d in [dir, other, support] {
        std::fs::remove_dir_all(&d).ok();
    }
}

#[test]
fn scripts_and_lisp_load_follow_the_same_rules() {
    let dir = folder("scripts");
    let scr = dir.join("draw.scr");
    std::fs::write(&scr, "circle 0,0 2\n").unwrap();
    let file = lsp(&dir, "tool.lsp");
    let mut s = Session::new();
    assert!(s.execute("script", &json!({ "path": scr.to_string_lossy() })).is_err());
    assert_eq!(s.doc().unwrap().model.len(), 0);
    assert_eq!(s.secureload_ask.as_ref().and_then(|a| a.retry.as_ref()).map(|r| r.0.as_str()), Some("script"));
    // (load …) from a routine: refused with the reason, nothing defined.
    let r = s.execute("lisp", &json!({ "code": format!("(load \"{}\")", file.replace('\\', "/")) })).unwrap();
    assert!(r["error"].as_str().unwrap_or_default().contains("trusted folder"), "{r}");
    assert_eq!(marker(&mut s), "nil");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn code_cannot_loosen_the_rules_or_write_code_files() {
    let dir = folder("code");
    let mut s = Session::new();
    for code in ["(setvar \"SECURELOAD\" 0)", "(setvar \"TRUSTEDPATHS\" \"/\")"] {
        let r = s.execute("lisp", &json!({ "code": code })).unwrap();
        assert!(r["error"].as_str().is_some_and(|e| e.contains("rejected")), "{code}: {r}");
    }
    let r = s.execute("lisp", &json!({ "code": "(command \"secureload.allow\" \"x.lsp\")" })).unwrap();
    let _ = r;
    assert_eq!(s.settings.secureload, 1);
    assert!(s.settings.trustedpaths.is_empty());
    // A script can't either.
    s.script("SECURELOAD 0\n").ok();
    assert_eq!(s.settings.secureload, 1, "a script can't turn it off");
    // Writing code files from a routine is refused; other files are fine.
    let virus = dir.join("acaddoc.lsp").to_string_lossy().replace('\\', "/");
    let r = s.execute("lisp", &json!({ "code": format!("(open \"{virus}\" \"w\")") })).unwrap();
    assert!(r["error"].as_str().unwrap_or_default().contains("can't write a .lsp"), "{r}");
    let txt = dir.join("notes.txt").to_string_lossy().replace('\\', "/");
    let r = s.execute("lisp", &json!({ "code": format!("(setq f (open \"{txt}\" \"w\")) (write-line \"ok\" f) (close f)") })).unwrap();
    assert!(r.get("error").is_none(), "{r}");
    assert!(dir.join("notes.txt").exists());
    // From the command line, by the user, it can be changed.
    s.execute("setvar", &json!({ "name": "SECURELOAD", "value": 2 })).unwrap();
    assert_eq!(s.settings.secureload, 2);
    std::fs::remove_dir_all(&dir).ok();
}
