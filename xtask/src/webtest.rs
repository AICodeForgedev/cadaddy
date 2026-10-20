//! `cargo xtask webtest`: the web build (apps/cadaddy-web) tested in a headless browser.
//!
//! Builds the wasm (dev profile) and its JavaScript glue (the `wasm-bindgen` CLI, the version in
//! Cargo.lock), makes the test files with `cadaddy-cli` (a floor plan as DXF and DWG, and its
//! walkthrough package), serves them on a local port, and opens `index.html?selftest&webgl` in a
//! headless Chromium-family browser. The page tests itself (apps/cadaddy-web/src/selftest.rs:
//! open DXF and DWG, draw and undo, save as a download, Drawing Recovery, the walkthrough in
//! Walk, autosave into browser storage) and sends the results back here. Skipped, not failed,
//! when there is no such browser (`CADADDY_CHROME` names one) or no matching `wasm-bindgen`.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;

/// How long the page has to test itself (autosave alone takes a minute).
const TIMEOUT: Duration = Duration::from_secs(300);

/// A Chromium-family browser: `CADADDY_CHROME`, Chrome or Chromium where they install, or one on
/// the PATH.
fn browser() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("CADADDY_CHROME").map(PathBuf::from).filter(|p| p.is_file()) {
        return Some(p);
    }
    let mut known = vec![
        PathBuf::from("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
        PathBuf::from("/Applications/Chromium.app/Contents/MacOS/Chromium"),
        PathBuf::from(r"C:\Program Files\Google\Chrome\Application\chrome.exe"),
    ];
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        known.push(home.join("Applications/Google Chrome.app/Contents/MacOS/Google Chrome"));
    }
    if let Some(p) = known.into_iter().find(|p| p.is_file()) {
        return Some(p);
    }
    for name in ["google-chrome", "google-chrome-stable", "chromium", "chromium-browser", "chrome"] {
        let found = Command::new("sh").args(["-c", &format!("command -v {name}")]).output().ok().filter(|o| o.status.success());
        if let Some(o) = found {
            let p = PathBuf::from(String::from_utf8_lossy(&o.stdout).trim());
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

/// The wasm-bindgen version Cargo.lock pins.
fn locked_bindgen(root: &Path) -> Option<String> {
    let lock = std::fs::read_to_string(root.join("Cargo.lock")).ok()?;
    let at = lock.find("name = \"wasm-bindgen\"\nversion = \"")?;
    let rest = lock.get(at + "name = \"wasm-bindgen\"\nversion = \"".len()..)?;
    rest.split('"').next().map(str::to_string)
}

fn skip(why: &str) -> Result<(), String> {
    println!("webtest: skipped ({why})");
    Ok(())
}

/// `%XX` and `+` decoded.
pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while let Some(&c) = b.get(i) {
        match c {
            b'%' => {
                let hex = b.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
                match hex {
                    Some(v) => {
                        out.push(v);
                        i += 3;
                        continue;
                    }
                    None => out.push(c),
                }
            }
            b'+' => out.push(b' '),
            _ => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The content type of a served file.
pub fn mime(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or_default() {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript",
        "wasm" => "application/wasm",
        "json" => "application/json",
        _ => "application/octet-stream",
    }
}

/// One request: a file under `dir`, or the page's report (`/selftest/result?data=`).
fn serve(stream: TcpStream, dir: &Path, report: &Mutex<Option<String>>) {
    let mut reader = BufReader::new(match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    });
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return;
    }
    // Skip the headers.
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h).map_or(true, |n| n == 0) || h == "\r\n" || h == "\n" {
            break;
        }
    }
    let target = line.split_whitespace().nth(1).unwrap_or("/");
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let mut w = stream;
    if path.ends_with("/selftest/result") {
        let data = query.split('&').find_map(|kv| kv.strip_prefix("data=")).map(percent_decode).unwrap_or_default();
        if let Ok(mut r) = report.lock() {
            *r = Some(data);
        }
        let _ = write!(w, "HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n");
        return;
    }
    let rel = percent_decode(path.trim_start_matches('/'));
    let rel = if rel.is_empty() { "index.html".to_string() } else { rel };
    let file = dir.join(&rel);
    let inside = !rel.split(['/', '\\']).any(|p| p == "..");
    match std::fs::read(&file).ok().filter(|_| inside) {
        Some(body) => {
            let _ = write!(w, "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", mime(&rel), body.len());
            let _ = w.write_all(&body);
        }
        None => {
            let _ = write!(w, "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        }
    }
}

pub fn run(root: &Path, _rest: &[&str]) -> Result<(), String> {
    let Some(chrome) = browser() else { return skip("no Chromium-family browser; set CADADDY_CHROME") };
    let Some(want) = locked_bindgen(root) else { return skip("no wasm-bindgen in Cargo.lock") };
    let have = Command::new("wasm-bindgen").arg("--version").output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    match &have {
        Some(v) if v.ends_with(&want) => {}
        Some(v) => return skip(&format!("{v} installed, Cargo.lock has {want}: cargo install wasm-bindgen-cli --version {want}")),
        None => return skip(&format!("no wasm-bindgen CLI: cargo install wasm-bindgen-cli --version {want}")),
    }
    let target = std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| root.join("target"), PathBuf::from);
    let site = target.join("webtest");
    let _ = std::fs::remove_dir_all(&site);
    std::fs::create_dir_all(site.join("selftest")).map_err(|e| e.to_string())?;
    // The wasm and its glue.
    let mut c = crate::cargo();
    c.args(["build", "-q", "-p", "cadaddy-web", "--target", "wasm32-unknown-unknown"]);
    crate::run(c, "cargo build -p cadaddy-web --target wasm32-unknown-unknown")?;
    let wasm = target.join("wasm32-unknown-unknown").join("debug").join("cadaddy-web.wasm");
    let mut b = Command::new("wasm-bindgen");
    b.args(["--target", "web", "--no-typescript", "--out-dir"]).arg(site.join("pkg")).arg(&wasm);
    crate::run(b, "wasm-bindgen --target web")?;
    let page = std::fs::read_to_string(root.join("apps/cadaddy-web/index.html")).map_err(|e| e.to_string())?;
    let page: String = page
        .lines()
        .map(|l| if l.contains("data-trunk") { r#"  <script type="module">import init from "./pkg/cadaddy-web.js"; init();</script>"# } else { l })
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(site.join("index.html"), page).map_err(|e| e.to_string())?;
    // The test files, made by the command-line tool.
    let mut cli = crate::cargo();
    cli.args(["build", "-q", "-p", "cadaddy-cli"]);
    crate::run(cli, "cargo build -p cadaddy-cli")?;
    let tool = target.join("debug").join(if cfg!(windows) { "cadaddy-cli.exe" } else { "cadaddy-cli" });
    let files = site.join("selftest");
    let plan = files.join("plan.dxf");
    let ok = |c: &mut Command, what: &str| -> Result<(), String> {
        let o = c.output().map_err(|e| format!("{what}: {e}"))?;
        if o.status.success() { Ok(()) } else { Err(format!("{what}: {}", String::from_utf8_lossy(&o.stderr).trim())) }
    };
    ok(Command::new(&tool).args(["sample", "floorplan"]).arg(&plan), "cadaddy-cli sample floorplan")?;
    ok(Command::new(&tool).arg("convert").arg(&plan).arg(files.join("plan.dwg")), "cadaddy-cli convert")?;
    let publish = format!("walk.publish {}", serde_json::json!({ "path": files.join("plan.cdywalk") }));
    ok(Command::new(&tool).arg("run").arg(&plan).args(["--cmd", &publish]), "cadaddy-cli walk.publish")?;
    // Serve the site.
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let report: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    {
        let (dir, report) = (site.clone(), report.clone());
        std::thread::spawn(move || {
            for conn in listener.incoming().flatten() {
                let (dir, report) = (dir.clone(), report.clone());
                std::thread::spawn(move || serve(conn, &dir, &report));
            }
        });
    }
    let profile = site.join("chrome-profile");
    let url = format!("http://127.0.0.1:{port}/index.html?selftest&webgl");
    println!("webtest: {} on {url}", chrome.display());
    let mut child = Command::new(&chrome)
        .args([
            "--headless=new",
            "--no-first-run",
            "--no-default-browser-check",
            "--window-size=1280,800",
            "--use-angle=swiftshader",
            "--enable-unsafe-swiftshader",
            "--ignore-gpu-blocklist",
        ])
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg(&url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("{}: {e}", chrome.display()))?;
    let start = Instant::now();
    let got = loop {
        if let Some(r) = report.lock().ok().and_then(|r| r.clone()) {
            break Some(r);
        }
        if start.elapsed() > TIMEOUT || matches!(child.try_wait(), Ok(Some(_))) {
            break None;
        }
        std::thread::sleep(Duration::from_millis(500));
    };
    let _ = child.kill();
    let _ = child.wait();
    let Some(text) = got else { return Err(format!("the page sent no results within {} s ({url})", TIMEOUT.as_secs())) };
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("results: {e}: {text}"))?;
    let mut failed = 0;
    for s in v["steps"].as_array().into_iter().flatten() {
        let ok = s["ok"] == true;
        failed += usize::from(!ok);
        println!("  {}  {}: {}", if ok { "PASS" } else { "FAIL" }, s["step"].as_str().unwrap_or_default(), s["detail"].as_str().unwrap_or_default());
    }
    println!("webtest: {}/{} passed in {:.0} s", v["passed"], v["total"], start.elapsed().as_secs_f64());
    let _ = std::fs::remove_dir_all(&profile);
    if failed > 0 { Err(format!("{failed} web step(s) failed")) } else { Ok(()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoding_and_types() {
        assert_eq!(percent_decode("%7B%22a%22%3A1%7D+x"), r#"{"a":1} x"#);
        assert_eq!(percent_decode("bad%zz"), "bad%zz");
        assert_eq!(mime("pkg/cadaddy-web_bg.wasm"), "application/wasm");
        assert_eq!(mime("index.html"), "text/html; charset=utf-8");
    }
}
