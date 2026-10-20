//! SPELL: misspelled words in text, mtext (format codes skipped), attributes, dimension text,
//! tables and multileaders are found with suggestions; Change, Change All, Ignore All and Add
//! work by JSON and at the command line; DCTMAIN and DCTCUST name the dictionaries.

use cadaddy_engine::Session;
use cadaddy_engine::doc::{EntityKind, Handle};
use serde_json::{Value, json};

fn h(v: &Value) -> Handle {
    let t = v["handle"].as_str().or_else(|| v["handles"][0].as_str()).unwrap();
    Handle(u64::from_str_radix(t, 16).unwrap())
}

fn text(s: &mut Session, y: f64, t: &str) -> Handle {
    h(&s.execute("text", &json!({"at": [0, y], "text": t, "height": 1})).unwrap())
}

fn value(s: &Session, h: Handle) -> String {
    match &s.doc().unwrap().entity(h).unwrap().kind {
        EntityKind::Text(t) => t.value.clone(),
        EntityKind::MText(t) => t.contents.clone(),
        k => panic!("{k:?}"),
    }
}

fn words(r: &Value) -> Vec<String> {
    r["misspelled"].as_array().unwrap().iter().map(|m| m["word"].as_str().unwrap().to_string()).collect()
}

/// The user dictionary setting is the program's, not a drawing's: the tests take turns.
static TURN: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// A user dictionary of its own for each test (held while the guard lives).
fn own_dictionary(s: &mut Session, name: &str) -> (std::path::PathBuf, std::sync::MutexGuard<'static, ()>) {
    let turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!("cadaddy-spell-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("custom.cus");
    let _ = std::fs::remove_file(&path);
    s.execute("setvar", &json!({"name": "DCTCUST", "value": path.to_str().unwrap()})).unwrap();
    (path, turn)
}

#[test]
fn misspellings_are_found_in_every_kind_of_text_with_suggestions() {
    let mut s = Session::new();
    let _turn = own_dictionary(&mut s, "find");
    text(&mut s, 0.0, "MOUNTING BRAKCET, 4 HOLES");
    s.execute("mtext", &json!({"at": [0, 10], "text": "{\\fArial|b1;Recieve} the\\Pdelivrey at 3pm", "height": 1, "width": 30})).unwrap();
    s.execute("text", &json!({"at": [0, 20], "text": "Panel shedule", "height": 1})).unwrap();
    let r = s.execute("spell", &json!({})).unwrap();
    let w = words(&r);
    assert_eq!(w, ["BRAKCET", "Recieve", "delivrey", "shedule"], "{r}");
    // Format codes (\fArial, \P) and the number with letters (3pm) are not words.
    let m = &r["misspelled"][0];
    assert!(m["suggestions"].as_array().unwrap().iter().any(|x| x == "BRACKET"), "UPPER case suggestions: {m}");
    let rec = &r["misspelled"][1];
    assert_eq!(rec["suggestions"][0], "Receive", "{rec}");
    assert!(r["misspelled"][3]["suggestions"].as_array().unwrap().iter().any(|x| x == "schedule"));

    // Attributes, dimension overrides and table cells.
    let mut s = Session::new();
    drop(_turn);
    let _turn = own_dictionary(&mut s, "find2");
    s.execute("attdef", &json!({"tag": "NAME", "default": "", "at": [0, 0], "height": 1})).unwrap();
    let hs: Vec<String> = s.doc().unwrap().model.iter().map(|e| e.handle.hex()).collect();
    s.execute("block", &json!({"name": "TAG", "base": [0, 0], "handles": hs, "keep": "delete"})).unwrap();
    s.execute("insert", &json!({"name": "TAG", "at": [0, 0], "attribs": {"NAME": "electirc room"}})).unwrap();
    s.execute("dimlinear", &json!({"p1": [0, 0], "p2": [10, 0], "at": [5, 5], "text": "<> typcial"})).unwrap();
    s.execute("table", &json!({"at": [50, 50], "rows": 1, "cols": 1, "cells": [["Conduit runn"]]})).unwrap();
    let r = s.execute("spell", &json!({"suggestions": false})).unwrap();
    let w = words(&r);
    assert_eq!(w, ["electirc", "typcial", "runn"], "{r}");
    let wheres: Vec<&Value> = r["misspelled"].as_array().unwrap().iter().map(|m| &m["where"]).collect();
    assert_eq!(wheres[0], &json!({"attribute": 0}));
    assert_eq!(wheres[1], &json!("dimension"));
    assert!(wheres[2]["cell"].is_array());
}

#[test]
fn change_and_change_all_fix_words_and_keep_codes() {
    let mut s = Session::new();
    let _turn = own_dictionary(&mut s, "change");
    let a = text(&mut s, 0.0, "Pannel A and pannel B");
    let b = text(&mut s, 5.0, "SEE PANNEL C");
    let m = h(&s.execute("mtext", &json!({"at": [0, 10], "text": "{\\C1;pannel} D", "height": 1, "width": 30})).unwrap());
    // One occurrence in one object.
    s.execute("spell", &json!({"action": "change", "handle": a.hex(), "word": "Pannel", "to": "Panel"})).unwrap();
    assert_eq!(value(&s, a), "Panel A and pannel B");
    // Every occurrence of that word, everywhere (exact word, so PANNEL stays for its own change).
    let r = s.execute("spell", &json!({"action": "changeAll", "word": "pannel", "to": "panel"})).unwrap();
    assert_eq!(r["changed"].as_array().unwrap().len(), 2, "{r}");
    assert_eq!(value(&s, a), "Panel A and panel B");
    assert_eq!(value(&s, m), "{\\C1;panel} D", "codes kept");
    assert_eq!(value(&s, b), "SEE PANNEL C");
    // Undo is one step per change.
    s.execute("undo", &json!({})).unwrap();
    assert_eq!(value(&s, m), "{\\C1;pannel} D");
    // Not found, missing target.
    assert!(s.execute("spell", &json!({"action": "change", "handle": b.hex(), "word": "nothere", "to": "x"})).is_err());
    assert!(s.execute("spell", &json!({"action": "change", "word": "PANNEL", "to": "PANEL"})).is_err());
}

#[test]
fn ignore_all_and_add_to_the_user_dictionary() {
    let mut s = Session::new();
    let (dict, _turn) = own_dictionary(&mut s, "add");
    text(&mut s, 0.0, "CADaddy uses Zorbulator and Quixplank parts");
    let w = words(&s.execute("spell", &json!({"suggestions": false})).unwrap());
    assert!(w.contains(&"Zorbulator".to_string()) && w.contains(&"Quixplank".to_string()), "{w:?}");
    s.execute("spell", &json!({"action": "ignoreAll", "word": "Zorbulator"})).unwrap();
    s.execute("spell", &json!({"action": "add", "word": "Quixplank"})).unwrap();
    let w = words(&s.execute("spell", &json!({"suggestions": false})).unwrap());
    assert!(!w.contains(&"Zorbulator".to_string()) && !w.contains(&"Quixplank".to_string()), "{w:?}");
    // Added words are in the DCTCUST file, read again after DCTCUST changes.
    let file = std::fs::read_to_string(&dict).unwrap();
    assert!(file.lines().any(|l| l == "Quixplank"), "{file}");
    assert_eq!(s.execute("getvar", &json!({"name": "DCTCUST"})).unwrap()["value"], dict.to_str().unwrap());
    assert!(s.execute("spell", &json!({"action": "add", "word": "two words"})).is_err());
    // DCTMAIN: American English unless the drawing chose another; codes not in the list refused.
    assert_eq!(s.execute("getvar", &json!({"name": "DCTMAIN"})).unwrap()["value"], "enu");
    assert!(s.execute("setvar", &json!({"name": "DCTMAIN", "value": "xx"})).is_err());
}

#[test]
fn spell_at_the_command_line_walks_the_misspellings() {
    let mut s = Session::new();
    let _turn = own_dictionary(&mut s, "cmdline");
    let a = text(&mut s, 0.0, "Wirring for the condiut and the condiut box, frobnitzle");
    s.cmdline("spell").unwrap();
    s.cmdline("all").unwrap();
    s.cmdline("").unwrap();
    assert!(s.prompt_text().contains("\"Wirring\" is not in the dictionary"), "{}", s.prompt_text());
    // Change: the first suggestion by default.
    s.cmdline("c").unwrap();
    assert!(s.prompt_text().contains("Change \"Wirring\" to <Wiring>"), "{}", s.prompt_text());
    s.cmdline("").unwrap();
    assert!(s.prompt_text().contains("\"condiut\""), "{}", s.prompt_text());
    // Change All: both condiut.
    s.cmdline("l").unwrap();
    s.cmdline("conduit").unwrap();
    assert!(s.prompt_text().contains("\"frobnitzle\""), "{}", s.prompt_text());
    s.cmdline("i").unwrap();
    assert!(s.running.is_none(), "{}", s.prompt_text());
    assert_eq!(value(&s, a), "Wiring for the conduit and the conduit box, frobnitzle");
}

#[test]
fn hostile_spell_requests_are_refused() {
    let mut s = Session::new();
    let _turn = own_dictionary(&mut s, "hostile");
    let a = text(&mut s, 0.0, "abc");
    for bad in [
        json!({"action": "explode"}),
        json!({"action": "change", "handle": a.hex(), "word": "abc"}),
        json!({"action": "change", "handle": a.hex(), "word": "abc", "to": "x", "where": {"cell": "x"}}),
        json!({"action": "change", "handle": a.hex(), "word": "abc", "to": "x".repeat(20_000)}),
        json!({"action": "add"}),
    ] {
        assert!(s.execute("spell", &bad).is_err(), "{bad}");
    }
    // Long runs and odd characters don't trip the checker.
    let long = "a".repeat(5000);
    text(&mut s, 5.0, &format!("{long} ’tis don’t \\U+00e9t\u{e9} %%c ä"));
    assert!(s.execute("spell", &json!({})).is_ok());
}

/// Other languages (Q146): dictionaries read from the dictionary folder, chosen per drawing by
/// DCTMAIN; one not installed says where to put it; the list shows what is installed.
#[test]
fn other_languages_come_from_the_dictionary_folder() {
    let mut s = Session::new();
    let (_dict, _turn) = own_dictionary(&mut s, "langs");
    let dir = std::env::temp_dir().join(format!("cadaddy-spell-{}-dicts", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    cadaddy_engine::cmd::spell::set_dictionary_dir(dir.to_str().unwrap());
    let list = s.execute("spell", &json!({"action": "dictionaries"})).unwrap();
    let installed =
        |list: &Value, code: &str| list["dictionaries"].as_array().unwrap().iter().find(|d| d["code"] == code).unwrap()["installed"].clone();
    assert_eq!(list["dictionaries"].as_array().unwrap().len(), 8, "{list}");
    assert_eq!((installed(&list, "enu"), installed(&list, "fr")), (json!(true), json!(false)), "{list}");
    assert_eq!(list["current"], "enu");
    // French chosen for this drawing, not installed yet: checking says where the file goes.
    text(&mut s, 0.0, "La maisonn est belle hello");
    s.execute("setvar", &json!({"name": "DCTMAIN", "value": "FR"})).unwrap();
    assert_eq!(s.execute("getvar", &json!({"name": "DCTMAIN"})).unwrap()["value"], "fr");
    let e = s.execute("spell", &json!({})).unwrap_err().to_string();
    assert!(e.contains("French") && e.contains("fr.txt") && e.contains(dir.to_str().unwrap()), "{e}");
    // Installed (a made-up list with a notice): French words right, English ones not.
    std::fs::write(dir.join("fr.txt"), "made-up test list\n---\nla\nmaison\nest\nbelle\nélectricité\n").unwrap();
    let r = s.execute("spell", &json!({})).unwrap();
    assert_eq!(words(&r), vec!["maisonn".to_string(), "hello".to_string()], "{r}");
    assert_eq!(r["misspelled"][0]["suggestions"][0], "maison", "{r}");
    assert_eq!(r["dictionary"], "fr");
    assert_eq!(installed(&s.execute("spell", &json!({"action": "dictionaries"})).unwrap(), "fr"), json!(true));
    // Another drawing keeps American English.
    s.execute("new", &json!({})).unwrap();
    assert_eq!(s.execute("getvar", &json!({"name": "DCTMAIN"})).unwrap()["value"], "enu");
    cadaddy_engine::cmd::spell::set_dictionary_dir("");
    let _ = std::fs::remove_dir_all(&dir);
}
