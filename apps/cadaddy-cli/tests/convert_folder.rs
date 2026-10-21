//! `cadaddy-cli convert IN_FOLDER OUT_FOLDER --to X --recurse` on awkward folders (Q206): nested
//! folders, read-only files and folders, names with spaces and non-Latin letters, a damaged file
//! in the middle. Every good drawing converts (under its own name, in its own subfolder), each
//! bad one is reported with a reason, and the exit status says something failed.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[cfg(unix)]
use serde_json::Value;

fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cadaddy-cli")).args(args).output().unwrap()
}

#[cfg(unix)]
fn ok(args: &[&str]) -> String {
    let out = cli(args);
    assert!(out.status.success(), "{args:?}: {:?}: {}", out.status, String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
}

fn folder(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cadaddy-cli-convertfolder-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn s(p: &Path) -> String {
    p.display().to_string()
}

/// The JSON summary a folder conversion printed, and whether it exited 0.
#[cfg(unix)]
fn convert(input: &Path, out: &Path, to: &str) -> (Value, bool, String) {
    let o = cli(&["convert", &s(input), &s(out), "--to", to, "--recurse"]);
    let text = String::from_utf8_lossy(&o.stdout).into_owned();
    let v = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{e}: {text}\n{}", String::from_utf8_lossy(&o.stderr)));
    (v, o.status.success(), String::from_utf8_lossy(&o.stderr).into_owned())
}

#[cfg(unix)]
fn list(v: &Value, key: &str, field: &str) -> Vec<String> {
    let mut l: Vec<String> = v[key].as_array().unwrap().iter().map(|x| x[field].as_str().unwrap_or_default().to_string()).collect();
    l.sort();
    l
}

#[cfg(unix)]
fn read_only(p: &Path, on: bool) {
    use std::os::unix::fs::PermissionsExt;
    let mode = match (p.is_dir(), on) {
        (true, true) => 0o555,
        (true, false) => 0o755,
        (false, true) => 0o444,
        (false, false) => 0o644,
    };
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).unwrap();
}

/// Whether this process ignores file permissions (root does), so read-only can't be tested.
#[cfg(unix)]
fn permissions_ignored(dir: &Path) -> bool {
    let probe = dir.join("probe");
    std::fs::create_dir_all(&probe).unwrap();
    read_only(&probe, true);
    let ignored = std::fs::write(probe.join("x"), b"x").is_ok();
    read_only(&probe, false);
    let _ = std::fs::remove_dir_all(&probe);
    ignored
}

#[cfg(unix)]
#[test]
fn awkward_folders_convert_everything_good_and_report_the_rest() {
    let dir = folder("awkward");
    if permissions_ignored(&dir) {
        return;
    }
    let input = dir.join("in");
    let deep = input.join("Level 1/второй уровень/第三層");
    std::fs::create_dir_all(&deep).unwrap();
    ok(&["sample", "floorplan", &s(&input.join("A Sample Floor Plan.dwg"))]);
    // A damaged DWG between two good ones (in name order).
    let mut bad = b"AC1032".to_vec();
    bad.extend((0..5000u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8));
    std::fs::write(input.join("B damaged.dwg"), &bad).unwrap();
    ok(&["sample", "bracket", &s(&input.join("C Project A.dxf"))]);
    ok(&["sample", "bracket", &s(&input.join("Level 1/План этажа – Ä.dwg"))]);
    ok(&["sample", "floorplan", &s(&deep.join("平面図 2.dxf"))]);
    ok(&["convert", &s(&input.join("C Project A.dxf")), &s(&deep.join("Détail ü.cdy"))]);
    // Not drawings: left alone.
    std::fs::write(input.join("notes.txt"), "made-up notes").unwrap();
    // Read-only inputs and a read-only subfolder still convert.
    read_only(&input.join("A Sample Floor Plan.dwg"), true);
    read_only(&deep.join("平面図 2.dxf"), true);
    read_only(&deep, true);

    let out = dir.join("out to cdy");
    let (v, success, stderr) = convert(&input, &out, "cdy");
    assert!(!success, "a failed drawing makes the exit status 1: {v}");
    assert!(stderr.contains("B damaged.dwg"), "the failure is reported: {stderr}");
    let failed = v["failed"].as_array().unwrap();
    assert_eq!(failed.len(), 1, "{v}");
    assert!(failed[0]["file"].as_str().unwrap().ends_with("B damaged.dwg"), "{v}");
    assert!(!failed[0]["error"].as_str().unwrap_or_default().trim().is_empty(), "a reason: {v}");
    assert_eq!(v["total"], 6, "{v}");
    let made = [
        "A Sample Floor Plan.cdy",
        "C Project A.cdy",
        "Level 1/План этажа – Ä.cdy",
        "Level 1/второй уровень/第三層/平面図 2.cdy",
        "Level 1/второй уровень/第三層/Détail ü.cdy",
    ];
    assert_eq!(list(&v, "converted", "to"), {
        let mut m: Vec<String> = made.iter().map(|m| s(&out.join(m))).collect();
        m.sort();
        m
    });
    for m in made {
        let f = out.join(m);
        ok(&["info", &s(&f)]);
    }
    assert!(!out.join("B damaged.cdy").exists(), "nothing written for the damaged file");
    assert!(!out.join("notes.cdy").exists());

    // Back to DWG from the .cdy folder: everything converts.
    let back = dir.join("back");
    let (v, success, stderr) = convert(&out, &back, "dwg");
    assert!(success, "{v} {stderr}");
    assert_eq!(v["converted"].as_array().unwrap().len(), 5, "{v}");

    // An output that can't be written fails on its own; the others convert.
    let again = dir.join("again");
    std::fs::create_dir_all(again.join("Level 1")).unwrap();
    read_only(&again.join("Level 1"), true);
    let (v, success, _) = convert(&input, &again, "dxf");
    assert!(!success);
    let failed = list(&v, "failed", "file");
    assert_eq!(failed.len(), 4, "the damaged file and the three under the read-only folder: {v}");
    assert!(failed.iter().any(|f| f.ends_with("План этажа – Ä.dwg")), "{v}");
    assert_eq!(list(&v, "converted", "to"), vec![s(&again.join("A Sample Floor Plan.dxf")), s(&again.join("C Project A.dxf"))]);
    for f in v["failed"].as_array().unwrap() {
        let e = f["error"].as_str().unwrap_or_default();
        assert!(!e.trim().is_empty(), "a reason: {f}");
        assert!(!e.contains("invalid parameters"), "a reason a person reads: {f}");
    }

    // A read-only output file already there: written over (the batch overwrites) or reported, never
    // silently left stale while the report says converted.
    let one = dir.join("one");
    let (v, ..) = convert(&input, &one, "cdy");
    let target = one.join("C Project A.cdy");
    let before = std::fs::read(&target).unwrap();
    std::fs::write(&target, b"stale").unwrap();
    read_only(&target, true);
    let (v2, ..) = convert(&input, &one, "cdy");
    let after = std::fs::read(&target).unwrap();
    let reported_converted = list(&v2, "converted", "to").contains(&s(&target));
    assert_eq!(reported_converted, after != b"stale", "the report matches the file: {v2}");
    if reported_converted {
        assert_eq!(after.len() > 100, before.len() > 100, "{v}");
    }

    for p in [&input.join("A Sample Floor Plan.dwg"), &deep.join("平面図 2.dxf"), &deep, &again.join("Level 1"), &target] {
        read_only(p, false);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A folder that isn't there, a file given as the folder, and an empty folder: refused with a
/// message, nothing written.
#[test]
fn bad_folders_are_refused() {
    let dir = folder("bad");
    let none = dir.join("missing folder");
    let o = cli(&["convert", &s(&none), &s(&dir.join("o")), "--to", "cdy"]);
    assert!(!o.status.success());
    let e = String::from_utf8_lossy(&o.stderr);
    assert!(e.contains("missing folder: no such file or folder"), "the missing folder is named: {e}");
    std::fs::create_dir_all(dir.join("empty")).unwrap();
    let o = cli(&["convert", &s(&dir.join("empty")), &s(&dir.join("o")), "--to", "cdy", "--recurse"]);
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("no drawings"), "{}", String::from_utf8_lossy(&o.stderr));
    let o = cli(&["convert", &s(&dir.join("empty")), &s(&dir.join("o")), "--to", "xyz"]);
    assert!(!o.status.success());
    assert!(!dir.join("o").exists(), "nothing made");
    let _ = std::fs::remove_dir_all(&dir);
}
