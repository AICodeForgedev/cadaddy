//! `cadaddy-cli plot IN OUT`: a folder of drawings plotted to one PDF each.

use std::process::Command;

fn cli(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_cadaddy-cli")).args(args).output().unwrap()
}

#[test]
fn a_folder_plots_to_pdfs() {
    let dir = std::env::temp_dir().join(format!("cadaddy-cli-plot-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let p = |n: &str| dir.join(n).display().to_string();
    for (sample, name) in [("floorplan", "plan.dxf"), ("bracket", "bracket.dwg")] {
        assert!(cli(&["sample", sample, &p(name)]).status.success());
    }
    let out = cli(&["plot", &p(""), &p("pdf"), "--layouts", "Model", "--stamp"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["plotted"].as_array().unwrap().len(), 2, "{v}");
    for n in ["plan.pdf", "bracket.pdf"] {
        assert!(std::fs::read(dir.join("pdf").join(n)).unwrap().starts_with(b"%PDF"), "{n}");
    }
    // A drawing that fails makes the exit status 1; the others are still plotted.
    std::fs::write(dir.join("zz.dxf"), "0\nSECTION\n2\nOOPS").unwrap();
    let out = cli(&["plot", &p(""), &p("pdf2")]);
    assert_eq!(out.status.code(), Some(1));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!((v["plotted"].as_array().unwrap().len(), v["failed"].as_array().unwrap().len()), (2, 1), "{v}");
    assert!(String::from_utf8_lossy(&cli(&["plot", &p(""), &p("x"), "--bogus"]).stderr).contains("unknown option"));
    let _ = std::fs::remove_dir_all(&dir);
}
