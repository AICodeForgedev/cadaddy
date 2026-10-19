//! `cadaddy-cli` and .cdy files: `convert` between .cdy, DWG, DXF and the image formats, one file
//! or a folder at a time; `info` with a .cdy file's chunk sizes; `verify` on sound and damaged files.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cadaddy-cli")).args(args).output().unwrap()
}

fn ok(args: &[&str]) -> String {
    let out = cli(args);
    assert!(out.status.success(), "{args:?}: {:?}: {}", out.status, String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
}

/// The first JSON document a command printed.
fn first_json(text: &str) -> Value {
    serde_json::Deserializer::from_str(text).into_iter::<Value>().next().unwrap().unwrap()
}

fn folder(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cadaddy-cli-cdy-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn p(dir: &Path, name: &str) -> String {
    dir.join(name).display().to_string()
}

fn objects(file: &str) -> u64 {
    let v: Vec<Value> = serde_json::Deserializer::from_str(&ok(&["info", file])).into_iter::<Value>().map(Result::unwrap).collect();
    let inspect = v.last().unwrap();
    inspect["entityCount"].as_u64().unwrap_or_else(|| panic!("{inspect}"))
}

#[test]
fn single_files_convert_to_and_from_cdy() {
    let dir = folder("one");
    ok(&["sample", "floorplan", &p(&dir, "plan.dxf")]);
    ok(&["sample", "bracket", &p(&dir, "bracket.dwg")]);
    let n = objects(&p(&dir, "plan.dxf"));
    assert!(n > 10, "{n}");
    // DXF → .cdy → DWG → .cdy, the objects all the way through.
    ok(&["convert", &p(&dir, "plan.dxf"), &p(&dir, "plan.cdy")]);
    assert!(std::fs::read(dir.join("plan.cdy")).unwrap().starts_with(b"CADDY"));
    ok(&["convert", &p(&dir, "plan.cdy"), &p(&dir, "plan.dwg")]);
    ok(&["convert", &p(&dir, "plan.dwg"), &p(&dir, "again.cdy")]);
    assert_eq!(objects(&p(&dir, "again.cdy")), n);
    ok(&["convert", &p(&dir, "bracket.dwg"), &p(&dir, "bracket.cdy")]);
    // .cdy to the image formats.
    for ext in ["png", "svg", "pdf"] {
        ok(&["convert", &p(&dir, "plan.cdy"), &p(&dir, &format!("plan.{ext}"))]);
        assert!(std::fs::metadata(dir.join(format!("plan.{ext}"))).unwrap().len() > 100, "{ext}");
    }
    // --to belongs to folders.
    let out = cli(&["convert", &p(&dir, "plan.dxf"), &p(&dir, "x.cdy"), "--to", "cdy"]);
    assert!(!out.status.success() && String::from_utf8_lossy(&out.stderr).contains("folder"));
    let out = cli(&["convert", &p(&dir, "plan.dxf"), &p(&dir, "x.cdy"), "--bogus"]);
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown option --bogus"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_folder_converts_and_a_bad_drawing_does_not_stop_the_rest() {
    let dir = folder("batch");
    let input = dir.join("in");
    std::fs::create_dir_all(&input).unwrap();
    ok(&["sample", "floorplan", &p(&input, "plan.dxf")]);
    ok(&["sample", "bracket", &p(&input, "bracket.DWG")]);
    std::fs::write(input.join("notes.txt"), "not a drawing").unwrap();
    let out = p(&dir, "out");
    let report: Value = serde_json::from_str(&ok(&["convert", &p(&dir, "in"), &out, "--to", "cdy"])).unwrap();
    assert_eq!(report["converted"].as_array().unwrap().len(), 2, "{report}");
    assert!(dir.join("out/plan.cdy").exists() && dir.join("out/bracket.cdy").exists());
    assert!(!dir.join("out/notes.cdy").exists(), "only drawings");
    // And back again, from .cdy files, to images.
    let report: Value = serde_json::from_str(&ok(&["convert", &out, &p(&dir, "png"), "--to", ".PNG"])).unwrap();
    assert_eq!(report["converted"].as_array().unwrap().len(), 2, "{report}");
    // A damaged drawing and a name clash fail; the others are still converted; exit status 1.
    std::fs::write(input.join("broken.dxf"), "0\nSECTION\n2\nENTITIES\n0\nLINE\n10\nnot a number").unwrap();
    std::fs::write(input.join("broken.dwg"), b"AC1032 garbage").unwrap();
    let run = cli(&["convert", &p(&dir, "in"), &p(&dir, "out2"), "--to", "cdy"]);
    assert_eq!(run.status.code(), Some(1));
    let report: Value = serde_json::from_slice(&run.stdout).unwrap();
    let failed: Vec<&str> = report["failed"].as_array().unwrap().iter().map(|f| f["file"].as_str().unwrap()).collect();
    assert!(failed.iter().any(|f| f.ends_with("broken.dwg")), "{report}");
    assert!(dir.join("out2/plan.cdy").exists() && dir.join("out2/bracket.cdy").exists());
    // Folder mistakes.
    let e = cli(&["convert", &p(&dir, "in"), &p(&dir, "out3")]);
    assert!(String::from_utf8_lossy(&e.stderr).contains("--to"));
    let e = cli(&["convert", &p(&dir, "in"), &p(&dir, "out3"), "--to", "docx"]);
    assert!(String::from_utf8_lossy(&e.stderr).contains("unknown format"));
    std::fs::create_dir_all(dir.join("empty")).unwrap();
    let e = cli(&["convert", &p(&dir, "empty"), &p(&dir, "out3"), "--to", "dxf"]);
    assert!(String::from_utf8_lossy(&e.stderr).contains("no drawings"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn info_shows_a_cdy_files_manifest_and_chunk_sizes() {
    let dir = folder("info");
    ok(&["sample", "floorplan", &p(&dir, "plan.dxf")]);
    ok(&["convert", &p(&dir, "plan.dxf"), &p(&dir, "plan.cdy")]);
    let v = first_json(&ok(&["info", &p(&dir, "plan.cdy")]));
    assert!(v["manifest"]["writer"].as_str().unwrap().starts_with("CADaddy"), "{v}");
    for tag in ["MANI", "HEAD", "TABL", "ENTS"] {
        assert!(v["chunks"][tag]["count"].as_u64().unwrap() >= 1, "{tag}: {v}");
        assert!(v["chunks"][tag]["raw"].as_u64().unwrap() > 0, "{tag}: {v}");
    }
    assert_eq!(v["waste"], 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn verify_passes_a_sound_file_and_finds_damage() {
    let dir = folder("verify");
    ok(&["sample", "floorplan", &p(&dir, "plan.dxf")]);
    ok(&["convert", &p(&dir, "plan.dxf"), &p(&dir, "plan.cdy")]);
    let good: Value = serde_json::from_str(&ok(&["verify", &p(&dir, "plan.cdy")])).unwrap();
    assert_eq!((good["ok"].clone(), good["index"].clone()), (Value::Bool(true), Value::from("sound")), "{good}");
    assert!(good["chunks"].as_array().unwrap().iter().all(|c| c.get("damage").is_none()));
    let bytes = std::fs::read(dir.join("plan.cdy")).unwrap();
    // A flipped byte inside the model space chunk's data.
    let ents = good["chunks"].as_array().unwrap().iter().find(|c| c["tag"] == "ENTS").unwrap();
    let at = ents["offset"].as_u64().unwrap() as usize + 28 + 10;
    let mut bad = bytes.clone();
    bad[at] ^= 0xff;
    std::fs::write(dir.join("bad.cdy"), &bad).unwrap();
    let run = cli(&["verify", &p(&dir, "bad.cdy")]);
    assert_eq!(run.status.code(), Some(1));
    let v: Value = serde_json::from_slice(&run.stdout).unwrap();
    assert_eq!(v["ok"], Value::Bool(false));
    let damaged: Vec<&Value> = v["chunks"].as_array().unwrap().iter().filter(|c| c.get("damage").is_some()).collect();
    assert_eq!(damaged.len(), 1, "{v}");
    assert_eq!(damaged[0]["tag"], "ENTS");
    assert!(String::from_utf8_lossy(&run.stderr).contains("damaged"));
    // Cut short: the index is gone.
    std::fs::write(dir.join("short.cdy"), &bytes[..bytes.len() * 2 / 3]).unwrap();
    let run = cli(&["verify", &p(&dir, "short.cdy")]);
    assert_eq!(run.status.code(), Some(1));
    let v: Value = serde_json::from_slice(&run.stdout).unwrap();
    assert_eq!(v["index"], "damaged");
    // Not a .cdy file at all.
    let run = cli(&["verify", &p(&dir, "plan.dxf")]);
    assert_eq!(run.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&run.stderr).contains("not a CADaddy drawing"));
    let _ = std::fs::remove_dir_all(&dir);
}
