//! `cargo xtask share-page`: a shared project page (Q350) — a folder any web server serves as it
//! is, from any sub-path, that shows a building model read-only in a browser or on a phone.
//!
//! ```text
//! cargo xtask share-page <project.cdy|.ifc> --out DIR [--title "Title"] [--allow-download] [--no-strip] [--debug]
//! cargo xtask share-page --sample --out DIR      (a made-up IFC project, names planted)
//! cargo xtask share-page --hotel --out DIR       (three wings of the synthetic hotel, 207 rooms)
//! ```
//!
//! It builds the web viewer (`cadaddy-web`, release unless `--debug`) and its JavaScript glue
//! (`wasm-bindgen`, the version in Cargo.lock), writes `index.html` (relative URLs only, no
//! external resources, `noindex,nofollow`, no Open Graph preview, a content security policy that
//! allows this site alone), and the model files with `share.export` (stripped of names unless
//! `--no-strip`; the strip report goes beside the folder, `DIR.strip-report.txt`). The passcode
//! gate is the web server's (see the nginx block in the task notes): the page shows "access
//! denied" when the model fetch is refused with 401 or 403.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The page (viewer, glue, model) for the sample project must stay within this (release).
pub const BUDGET_RELEASE_BYTES: u64 = 40 << 20;
/// A dev-profile wasm is several times larger.
pub const BUDGET_DEBUG_BYTES: u64 = 400 << 20;

pub struct Options {
    pub source: Source,
    pub out: PathBuf,
    pub title: String,
    pub allow_download: bool,
    pub strip: bool,
    pub release: bool,
}

pub enum Source {
    File(PathBuf),
    Sample,
    Hotel,
}

fn usage() -> String {
    "usage: cargo xtask share-page (<project.cdy|.ifc> | --sample | --hotel) --out DIR [--title TITLE] [--allow-download] [--no-strip] [--debug]"
        .into()
}

pub fn parse(rest: &[&str]) -> Result<Options, String> {
    let mut source = None;
    let (mut out, mut title) = (None, None);
    let (mut allow_download, mut strip, mut release) = (false, true, true);
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match *a {
            "--out" => out = it.next().map(PathBuf::from),
            "--title" => title = it.next().map(|t| t.to_string()),
            "--allow-download" => allow_download = true,
            "--no-strip" => strip = false,
            "--strip" => strip = true,
            "--debug" => release = false,
            "--sample" => source = Some(Source::Sample),
            "--hotel" => source = Some(Source::Hotel),
            f if !f.starts_with("--") => source = Some(Source::File(PathBuf::from(f))),
            other => return Err(format!("share-page: unknown option {other}\n{}", usage())),
        }
    }
    let source = source.ok_or_else(usage)?;
    let out = out.ok_or_else(usage)?;
    let title = title.unwrap_or_else(|| match &source {
        Source::Hotel => "Sample Hotel".into(),
        Source::Sample => "Sample Wing".into(),
        Source::File(_) => "Shared Model".into(),
    });
    Ok(Options { source, out, title, allow_download, strip, release })
}

/// `&`, `<`, `>` and `"` escaped for HTML text and attributes.
pub fn html_escape(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '&' => "&amp;".to_string(),
            '<' => "&lt;".to_string(),
            '>' => "&gt;".to_string(),
            '"' => "&quot;".to_string(),
            '\'' => "&#39;".to_string(),
            c => c.to_string(),
        })
        .collect()
}

/// The page: the viewer started on `model/share.json`, the overview while it loads.
pub fn index_html(title: &str) -> String {
    let title = html_escape(title);
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1, user-scalable=no">
  <meta name="robots" content="noindex,nofollow">
  <meta name="referrer" content="no-referrer">
  <meta http-equiv="Content-Security-Policy" content="default-src 'self'; script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; connect-src 'self'; frame-ancestors 'self'; base-uri 'none'; form-action 'none'">
  <title>{title}</title>
  <style>
    html, body {{ margin: 0; padding: 0; width: 100%; height: 100%; overflow: hidden; background: #22282f; }}
    #cadaddy_canvas {{ position: absolute; inset: 0; width: 100%; height: 100%; display: block; }}
    #cadaddy_loading {{ position: absolute; inset: 0; display: flex; flex-direction: column; align-items: center; justify-content: center;
      color: #b8b8b8; font: 14px system-ui, sans-serif; pointer-events: none; }}
    #cadaddy_loading img {{ width: min(60vw, 320px); height: auto; opacity: 0.7; }}
  </style>
</head>
<body data-share="model/share.json">
  <canvas id="cadaddy_canvas"></canvas>
  <div id="cadaddy_loading"><img src="thumbs/overview.svg" alt=""><p>Loading {title}&hellip;</p></div>
  <script type="module">import init from "./pkg/cadaddy-web.js"; init();</script>
</body>
</html>
"#
    )
}

/// Every file under `dir` (relative path, size).
pub fn files(dir: &Path) -> Vec<(String, u64)> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<(String, u64)>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(base, &p, out);
            } else if let Ok(m) = e.metadata() {
                let rel = p.strip_prefix(base).unwrap_or(&p).to_string_lossy().replace('\\', "/");
                out.push((rel, m.len()));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

/// The made-up names the sample project plants (`crates/engine/src/sample_ifc.rs`, `PLANTED`).
pub fn planted(root: &Path) -> Result<Vec<String>, String> {
    let text = std::fs::read_to_string(root.join("crates/engine/src/sample_ifc.rs")).map_err(|e| e.to_string())?;
    let at = text.find("pub const PLANTED").ok_or("no PLANTED in sample_ifc.rs")?;
    let list = text.get(at..).and_then(|t| t.split_once("= [")).map(|(_, r)| r).and_then(|r| r.split_once("];")).map(|(l, _)| l).ok_or("PLANTED")?;
    let names: Vec<String> = list.split('"').skip(1).step_by(2).map(str::to_string).collect();
    if names.is_empty() { Err("PLANTED is empty".into()) } else { Ok(names) }
}

fn tool(root: &Path, name: &str) -> PathBuf {
    let target = std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| root.join("target"), PathBuf::from);
    target.join("debug").join(if cfg!(windows) { format!("{name}.exe") } else { name.to_string() })
}

fn ok(c: &mut Command, what: &str) -> Result<String, String> {
    eprintln!("$ {what}");
    let o = c.output().map_err(|e| format!("{what}: {e}"))?;
    if o.status.success() {
        Ok(String::from_utf8_lossy(&o.stdout).into_owned())
    } else {
        Err(format!("{what}: {}{}", String::from_utf8_lossy(&o.stderr).trim(), String::from_utf8_lossy(&o.stdout).trim()))
    }
}

/// The viewer and its glue into `out/pkg`.
fn viewer(root: &Path, out: &Path, release: bool) -> Result<(), String> {
    let want = crate::webtest::locked_bindgen(root).ok_or("no wasm-bindgen in Cargo.lock")?;
    let have = Command::new("wasm-bindgen").arg("--version").output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    if !have.as_deref().is_some_and(|v| v.ends_with(&want)) {
        return Err(format!("wasm-bindgen {want} is needed: cargo install wasm-bindgen-cli --version {want}"));
    }
    let mut c = crate::cargo();
    c.args(["build", "-q", "-p", "cadaddy-web", "--target", "wasm32-unknown-unknown"]);
    if release {
        c.arg("--release");
    }
    crate::run(c, "cargo build -p cadaddy-web --target wasm32-unknown-unknown")?;
    let target = std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| root.join("target"), PathBuf::from);
    let wasm = target.join("wasm32-unknown-unknown").join(if release { "release" } else { "debug" }).join("cadaddy-web.wasm");
    let mut b = Command::new("wasm-bindgen");
    b.args(["--target", "web", "--no-typescript", "--out-dir"]).arg(out.join("pkg")).arg(&wasm);
    if release {
        b.arg("--remove-name-section").arg("--remove-producers-section");
    }
    crate::run(b, "wasm-bindgen --target web")
}

/// The model files into `out` (`model/`, `thumbs/`).
fn model(root: &Path, o: &Options) -> Result<String, String> {
    let out = o.out.display().to_string();
    match &o.source {
        Source::Hotel => {
            let mut c = crate::cargo();
            c.args(["run", "-q", "-p", "cadaddy-mepfix", "--bin", "mep-fixtures", "--", "--hotel", "--share", &out]);
            ok(&mut c, "mep-fixtures --hotel --share")
        }
        Source::Sample | Source::File(_) => {
            let mut cli = crate::cargo();
            cli.args(["build", "-q", "-p", "cadaddy-cli"]);
            crate::run(cli, "cargo build -p cadaddy-cli")?;
            let cli = tool(root, "cadaddy-cli");
            let src = match &o.source {
                Source::File(f) => f.clone(),
                _ => {
                    let f = o
                        .out
                        .with_file_name(format!("{}-sample.ifc", o.out.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
                    ok(Command::new(&cli).args(["sample", "shareproject"]).arg(&f), "cadaddy-cli sample shareproject")?;
                    f
                }
            };
            let params = serde_json::json!({
                "dir": out, "title": o.title, "strip": o.strip, "allow_download": o.allow_download, "source": src.display().to_string(),
            });
            let cmd = format!("share.export {params}");
            ok(Command::new(&cli).arg("run").arg(&src).args(["--cmd", &cmd]), "cadaddy-cli run share.export")
        }
    }
}

/// Builds the page; returns its files and total size.
pub fn build(root: &Path, o: &Options) -> Result<(Vec<(String, u64)>, u64), String> {
    if let Source::File(f) = &o.source
        && !f.is_file()
    {
        return Err(format!("{}: no such file", f.display()));
    }
    let _ = std::fs::remove_dir_all(&o.out);
    std::fs::create_dir_all(&o.out).map_err(|e| format!("{}: {e}", o.out.display()))?;
    viewer(root, &o.out, o.release)?;
    let said = model(root, o)?;
    if !said.trim().is_empty() {
        eprintln!("{}", said.trim());
    }
    std::fs::write(o.out.join("index.html"), index_html(&o.title)).map_err(|e| e.to_string())?;
    let all = files(&o.out);
    let total = all.iter().map(|(_, n)| n).sum();
    Ok((all, total))
}

pub fn run(root: &Path, rest: &[&str]) -> Result<(), String> {
    let o = parse(rest)?;
    let (all, total) = build(root, &o)?;
    for (f, n) in &all {
        println!("  {n:>10}  {f}");
    }
    let budget = if o.release { BUDGET_RELEASE_BYTES } else { BUDGET_DEBUG_BYTES };
    println!("share-page: {} files, {:.1} MB in {} (budget {} MB)", all.len(), total as f64 / 1048576.0, o.out.display(), budget >> 20);
    if o.strip {
        println!("strip report (keep it private, it is not in the page): {}.strip-report.txt", o.out.display());
    }
    if total > budget { Err(format!("the page is {total} bytes, over the budget of {budget}")) } else { Ok(()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_has_relative_urls_only_and_asks_not_to_be_indexed() {
        let page = index_html("A <b>\"title\"</b> & co");
        assert!(page.contains(r#"<meta name="robots" content="noindex,nofollow">"#));
        assert!(page.contains("A &lt;b&gt;&quot;title&quot;&lt;/b&gt; &amp; co"));
        assert!(!page.contains("og:") && !page.contains("twitter:"), "no preview cards");
        for attr in ["src=\"", "href=\"", "from \"", "data-share=\""] {
            for (i, _) in page.match_indices(attr) {
                let url = page.get(i + attr.len()..).and_then(|r| r.split('"').next()).unwrap_or_default();
                assert!(!url.starts_with('/') && !url.contains("://"), "{attr}{url}");
            }
        }
        assert!(page.contains("connect-src 'self'"));
    }

    #[test]
    fn options_and_planted_names() {
        let o = parse(&["--sample", "--out", "x", "--allow-download", "--no-strip"]).unwrap();
        assert!(matches!(o.source, Source::Sample) && o.allow_download && !o.strip && o.release && o.title == "Sample Wing");
        assert!(parse(&["--out", "x"]).is_err());
        assert!(parse(&["f.ifc", "--bogus"]).is_err());
        let names = planted(&crate::root()).unwrap();
        assert!(names.len() >= 6 && names.iter().any(|n| n == "Quillonby"), "{names:?}");
    }
}
