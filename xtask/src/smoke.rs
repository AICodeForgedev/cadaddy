//! `cargo xtask smoke <app> [--version X.Y.Z] [--out DIR] [--keep]`: the release smoke test, so every
//! release can repeat it. `<app>` is what a user downloads or installs:
//! - a macOS `.dmg` (mounted read-only, its app run), a `.app` bundle or the app's executable: the
//!   desktop app, driven through its control channel (`--control PORT`, JSON lines on 127.0.0.1);
//! - a `.mcpb` (unpacked) or a `cadaddy-cli` executable: the MCP server (`cadaddy-cli mcp`, JSON-RPC
//!   on stdin/stdout).
//!
//! Steps: the version (Info.plist and Help ▸ About for the app, the MCP server's own version for
//! the CLI), draw, save as DWG, DXF and .cdy, reopen each, plot a PDF, sheet set publish, and in the
//! app Walk on/off and Help ▸ Check for Updates (with the release feed's latest version, "up to
//! date" when it is this one). The app runs with its own settings folder (CADADDY_CONFIG_DIR) so a
//! user's profile is never touched. Screenshots and the files made go to `--out` (default
//! `target/smoke`), with `report.json`; any failed step fails the command.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// The release feed (the app's own updater reads the same).
const LATEST: &str = "https://api.github.com/repos/AICodeForgedev/cadaddy/releases/latest";

#[derive(Debug, PartialEq)]
pub struct Opts {
    pub target: PathBuf,
    pub version: Option<String>,
    pub out: Option<PathBuf>,
    pub keep: bool,
}

pub fn parse(rest: &[&str]) -> Result<Opts, String> {
    let mut target = None;
    let mut opts = Opts { target: PathBuf::new(), version: None, out: None, keep: false };
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match *a {
            "--version" => opts.version = Some(it.next().ok_or("--version needs a value")?.trim_start_matches('v').to_string()),
            "--out" => opts.out = Some(PathBuf::from(it.next().ok_or("--out needs a folder")?)),
            "--keep" => opts.keep = true,
            s if s.starts_with("--") => return Err(format!("unknown option `{s}`")),
            s => {
                if target.replace(PathBuf::from(s)).is_some() {
                    return Err("one app at a time".into());
                }
            }
        }
    }
    opts.target = target.ok_or(
        "usage: cargo xtask smoke <CADaddy-Mac.dmg | CADaddy.app | cadaddy | CADaddy.mcpb | cadaddy-cli> [--version X.Y.Z] [--out DIR] [--keep]",
    )?;
    Ok(opts)
}

/// What is being tested.
#[derive(Debug, PartialEq)]
pub enum Kind {
    Dmg,
    App,
    Mcpb,
    Cli,
}

pub fn kind_of(p: &Path) -> Kind {
    let name = p.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    if name.ends_with(".dmg") {
        Kind::Dmg
    } else if name.ends_with(".mcpb") {
        Kind::Mcpb
    } else if name.starts_with("cadaddy-cli") {
        Kind::Cli
    } else {
        Kind::App
    }
}

/// One step's outcome.
#[derive(Debug)]
struct Step {
    name: &'static str,
    result: Result<String, String>,
}

/// The steps run so far, printed as they finish.
#[derive(Default)]
struct Report {
    steps: Vec<Step>,
}

impl Report {
    fn step(&mut self, name: &'static str, f: impl FnOnce() -> Result<String, String>) {
        let result = f();
        match &result {
            Ok(m) => println!("  PASS  {name:<16} {m}"),
            Err(e) => println!("  FAIL  {name:<16} {e}"),
        }
        self.steps.push(Step { name, result });
    }
    fn skip(&mut self, name: &'static str, why: &str) {
        println!("  SKIP  {name:<16} {why}");
        self.steps.push(Step { name, result: Ok(format!("skipped: {why}")) });
    }
    fn failed(&self) -> usize {
        self.steps.iter().filter(|s| s.result.is_err()).count()
    }
    fn json(&self, what: &str, version: &str) -> Value {
        json!({
            "target": what, "version": version,
            "steps": self.steps.iter().map(|s| match &s.result {
                Ok(m) => json!({ "step": s.name, "ok": true, "detail": m }),
                Err(e) => json!({ "step": s.name, "ok": false, "detail": e }),
            }).collect::<Vec<_>>(),
        })
    }
}

/// A file that a step made: present and starting with `magic` (when given).
fn made(path: &Path, magic: Option<&[u8]>) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if bytes.is_empty() {
        return Err(format!("{} is empty", path.display()));
    }
    if let Some(m) = magic
        && !bytes.starts_with(m)
    {
        return Err(format!("{} doesn't start with {:?}", path.display(), String::from_utf8_lossy(m)));
    }
    Ok(format!("{} ({} bytes)", path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), bytes.len()))
}

/// The release feed's latest version (`v` dropped), through curl (no HTTP code of our own).
fn latest_release() -> Result<String, String> {
    let out = Command::new("curl")
        .args(["-sSfL", "--max-time", "20", "-H", "Accept: application/vnd.github+json", LATEST])
        .output()
        .map_err(|e| format!("curl: {e}"))?;
    if !out.status.success() {
        return Err(format!("curl {LATEST}: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    let v: Value = serde_json::from_slice(&out.stdout).map_err(|e| format!("release feed: {e}"))?;
    v["tag_name"].as_str().map(|t| t.trim_start_matches('v').to_string()).ok_or_else(|| "release feed has no tag_name".into())
}

// ---------------------------------------------------------------- the app

/// The control channel of a running app.
pub(crate) struct Control {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
    id: u64,
}

impl Control {
    /// Connect to the app on `port`, presenting the token it writes under the settings folder
    /// `cfg` (control/<port>.token) when it has one (builds from before tokens write none).
    pub(crate) fn connect(port: u16, cfg: &Path, child: &mut Child, wait: Duration) -> Result<Control, String> {
        let start = Instant::now();
        loop {
            if let Ok(s) = TcpStream::connect(("127.0.0.1", port)) {
                s.set_read_timeout(Some(Duration::from_secs(120))).map_err(|e| e.to_string())?;
                let writer = s.try_clone().map_err(|e| e.to_string())?;
                let mut c = Control { reader: BufReader::new(s), writer, id: 0 };
                // The app writes the token just after it starts listening.
                let file = cfg.join("control").join(format!("{port}.token"));
                let token = (0..12).find_map(|_| {
                    let t = std::fs::read_to_string(&file).ok().map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
                    if t.is_none() {
                        std::thread::sleep(Duration::from_millis(250));
                    }
                    t
                });
                if let Some(token) = token {
                    c.call("auth", json!({ "token": token }))?;
                }
                return Ok(c);
            }
            if let Ok(Some(status)) = child.try_wait() {
                return Err(format!("the app exited before its control channel opened ({status})"));
            }
            if start.elapsed() > wait {
                return Err(format!("no control channel on port {port} after {}s", wait.as_secs()));
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }
    pub(crate) fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.id += 1;
        let line = json!({ "id": self.id, "method": method, "params": params }).to_string();
        writeln!(self.writer, "{line}").map_err(|e| format!("{method}: {e}"))?;
        let mut reply = String::new();
        self.reader.read_line(&mut reply).map_err(|e| format!("{method}: {e}"))?;
        let v: Value = serde_json::from_str(&reply).map_err(|e| format!("{method}: bad reply {reply:?}: {e}"))?;
        if v["ok"] == false || v.get("error").is_some_and(|e| !e.is_null()) {
            return Err(format!("{method}: {}", v.get("error").unwrap_or(&v)));
        }
        Ok(v.get("result").cloned().unwrap_or(Value::Null))
    }
    pub(crate) fn exec(&mut self, command: &str, params: Value) -> Result<Value, String> {
        self.call("engine.execute", json!({ "command": command, "params": params }))
    }
    fn entities(&mut self) -> Result<u64, String> {
        let r = self.exec("drawing.inspect", json!({ "entities": false }))?;
        r["entityCount"].as_u64().ok_or_else(|| format!("drawing.inspect has no entityCount: {r}"))
    }
}

/// A free local port.
pub(crate) fn free_port() -> Result<u16, String> {
    let l = std::net::TcpListener::bind(("127.0.0.1", 0)).map_err(|e| e.to_string())?;
    l.local_addr().map(|a| a.port()).map_err(|e| e.to_string())
}

/// A mounted disk image, detached when dropped.
struct Mounted(PathBuf);

impl Drop for Mounted {
    fn drop(&mut self) {
        let _ = Command::new("hdiutil").arg("detach").arg(&self.0).arg("-quiet").status();
    }
}

/// The executable inside a `.app` bundle.
fn bundle_exe(app: &Path) -> Result<PathBuf, String> {
    let dir = app.join("Contents").join("MacOS");
    std::fs::read_dir(&dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.is_file())
        .ok_or_else(|| format!("no executable in {}", dir.display()))
}

/// CFBundleShortVersionString of a `.app` bundle.
fn bundle_version(app: &Path) -> Result<String, String> {
    let plist = std::fs::read_to_string(app.join("Contents").join("Info.plist")).map_err(|e| format!("Info.plist: {e}"))?;
    plist_value(&plist, "CFBundleShortVersionString").ok_or_else(|| "Info.plist has no CFBundleShortVersionString".into())
}

/// `<key>K</key><string>V</string>` in an XML property list.
pub fn plist_value(plist: &str, key: &str) -> Option<String> {
    let at = plist.find(&format!("<key>{key}</key>"))?;
    let rest = plist.get(at..)?;
    let s = rest.find("<string>")? + "<string>".len();
    let e = rest.get(s..)?.find("</string>")? + s;
    rest.get(s..e).map(str::to_string)
}

fn run_app(exe: &Path, bundle_ver: Option<String>, want: &str, out: &Path, r: &mut Report) -> Result<(), String> {
    let cfg = out.join("config");
    std::fs::create_dir_all(&cfg).map_err(|e| e.to_string())?;
    let port = free_port()?;
    let log = std::fs::File::create(out.join("app.log")).map_err(|e| e.to_string())?;
    let mut child = Command::new(exe)
        .arg("--control")
        .arg(port.to_string())
        .env("CADADDY_CONFIG_DIR", &cfg)
        .stdout(log.try_clone().map_err(|e| e.to_string())?)
        .stderr(log)
        .spawn()
        .map_err(|e| format!("{}: {e}", exe.display()))?;
    let result = (|| -> Result<(), String> {
        let mut c = Control::connect(port, &cfg, &mut child, Duration::from_secs(90))?;
        r.step("launch", || c.call("ping", json!({})).map(|v| format!("control channel on {port}: {v}")));
        r.step("version", || match &bundle_ver {
            Some(v) if v == want => Ok(format!("Info.plist {v}")),
            Some(v) => Err(format!("Info.plist says {v}, expected {want}")),
            None => Ok(format!("expected {want} (no bundle to read; see about.png)")),
        });
        r.step("about", || {
            c.exec("ui.dialog.about", json!({}))?;
            std::thread::sleep(Duration::from_millis(800));
            let shot = out.join("about.png");
            c.call("ui.screenshot", json!({ "path": shot }))?;
            c.exec("ui.dialog.close", json!({})).ok();
            made(&shot, Some(b"\x89PNG")).map(|m| format!("Help ▸ About shown: {m} (check it says {want})"))
        });
        let mut drawn = 0;
        r.step("draw", || {
            let before = c.entities()?;
            c.call("cmdline.input", json!({ "text": "line 0,0 100,0 100,50" }))?;
            c.call("cmdline.input", json!({ "text": "" }))?;
            c.exec("circle", json!({ "center": [50, 25], "radius": 10 }))?;
            drawn = c.entities()?;
            if drawn >= before + 3 {
                Ok(format!("{before} → {drawn} objects"))
            } else {
                Err(format!("{before} → {drawn} objects (expected 3 more)"))
            }
        });
        for ext in ["dwg", "dxf", "cdy"] {
            let path = out.join(format!("smoke.{ext}"));
            let name = match ext {
                "dwg" => "save dwg",
                "dxf" => "save dxf",
                _ => "save cdy",
            };
            r.step(name, || {
                c.exec("saveas", json!({ "path": path }))?;
                made(&path, None)
            });
        }
        for ext in ["dwg", "dxf", "cdy"] {
            let path = out.join(format!("smoke.{ext}"));
            let name = match ext {
                "dwg" => "reopen dwg",
                "dxf" => "reopen dxf",
                _ => "reopen cdy",
            };
            r.step(name, || {
                c.exec("open", json!({ "path": path }))?;
                let n = c.entities()?;
                if n == drawn { Ok(format!("{n} objects")) } else { Err(format!("{n} objects, saved {drawn}")) }
            });
        }
        let pdf = out.join("smoke.pdf");
        r.step("plot pdf", || {
            c.exec("plot", json!({ "path": pdf }))?;
            made(&pdf, Some(b"%PDF"))
        });
        r.step("walk", || {
            c.exec("ui.walk", json!({}))?;
            std::thread::sleep(Duration::from_millis(1500));
            let shot = out.join("walk.png");
            c.call("ui.screenshot", json!({ "path": shot }))?;
            c.exec("ui.walk", json!({}))?;
            made(&shot, Some(b"\x89PNG")).map(|m| format!("on and off: {m}"))
        });
        let set = out.join("smoke.sheetset.json");
        let book = out.join("sheets.pdf");
        r.step("sheet set", || {
            c.exec("sheetset", json!({ "set": set, "action": "new", "name": "Smoke", "overwrite": true }))?;
            c.exec("sheetset", json!({ "set": set, "action": "add", "drawing": out.join("smoke.dwg") }))?;
            c.exec("sheetset", json!({ "set": set, "action": "publish", "path": book }))?;
            made(&book, Some(b"%PDF"))
        });
        r.step("update check", || {
            c.exec("ui.update.check", json!({}))?;
            std::thread::sleep(Duration::from_secs(5));
            let shot = out.join("update.png");
            c.call("ui.screenshot", json!({ "path": shot }))?;
            let latest = latest_release()?;
            if latest == want {
                Ok(format!("the release feed's latest is {latest}: up to date"))
            } else {
                Err(format!("the release feed's latest is {latest}, this is {want}"))
            }
        });
        r.step("quit", || {
            c.call("app.quit", json!({ "discard": true })).or_else(|_| c.exec("ui.quit", json!({}))).ok();
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(20) {
                if let Ok(Some(s)) = child.try_wait() {
                    return Ok(format!("exited ({s})"));
                }
                // A "Save changes?" question: discard (every drawing was saved under another name).
                if let Ok(p) = c.call("app.closePrompt", json!({}))
                    && !p["title"].is_null()
                {
                    c.call("app.answerClose", json!({ "choice": "discard" })).ok();
                }
                std::thread::sleep(Duration::from_millis(250));
            }
            Err("still running 20 s after quitting".into())
        });
        Ok(())
    })();
    if let Ok(None) = child.try_wait() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

// ---------------------------------------------------------------- the MCP server

pub(crate) struct Mcp {
    child: Child,
    reader: BufReader<std::process::ChildStdout>,
    id: u64,
}

impl Mcp {
    pub(crate) fn start(exe: &Path, cfg: &Path) -> Result<Mcp, String> {
        let mut child = Command::new(exe)
            .arg("mcp")
            .env("CADADDY_CONFIG_DIR", cfg)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("{}: {e}", exe.display()))?;
        let out = child.stdout.take().ok_or("no stdout")?;
        Ok(Mcp { child, reader: BufReader::new(out), id: 0 })
    }
    pub(crate) fn send(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.id += 1;
        let line = json!({ "jsonrpc": "2.0", "id": self.id, "method": method, "params": params }).to_string();
        let stdin = self.child.stdin.as_mut().ok_or("no stdin")?;
        writeln!(stdin, "{line}").map_err(|e| format!("{method}: {e}"))?;
        stdin.flush().map_err(|e| e.to_string())?;
        loop {
            let mut reply = String::new();
            if self.reader.read_line(&mut reply).map_err(|e| format!("{method}: {e}"))? == 0 {
                return Err(format!("{method}: the server closed its output"));
            }
            let v: Value = match serde_json::from_str(&reply) {
                Ok(v) => v,
                Err(_) => continue,
            };
            // Notifications have no id; replies carry ours.
            if v["id"] != json!(self.id) {
                continue;
            }
            if let Some(e) = v.get("error") {
                return Err(format!("{method}: {e}"));
            }
            return Ok(v["result"].clone());
        }
    }
    pub(crate) fn tool(&mut self, name: &str, args: Value) -> Result<Value, String> {
        self.tool_inner(name, args)
    }
    /// The server's process id.
    pub(crate) fn pid(&self) -> u32 {
        self.child.id()
    }
    fn tool_inner(&mut self, name: &str, args: Value) -> Result<Value, String> {
        let r = self.send("tools/call", json!({ "name": name, "arguments": args }))?;
        let text = r["content"][0]["text"].as_str().unwrap_or_default().to_string();
        if r["isError"] == true {
            return Err(format!("{name}: {text}"));
        }
        Ok(serde_json::from_str(&text).unwrap_or(Value::String(text)))
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        drop(self.child.stdin.take());
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn run_cli(exe: &Path, want: &str, out: &Path, r: &mut Report) -> Result<(), String> {
    let cfg = out.join("config");
    std::fs::create_dir_all(&cfg).map_err(|e| e.to_string())?;
    let mut m = Mcp::start(exe, &cfg)?;
    let mut server = String::new();
    r.step("launch", || {
        let init = m.send(
            "initialize",
            json!({ "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "xtask-smoke", "version": "1" } }),
        )?;
        server = init["serverInfo"]["version"].as_str().unwrap_or_default().to_string();
        let tools = m.send("tools/list", json!({}))?;
        Ok(format!("MCP server, {} tools", tools["tools"].as_array().map_or(0, Vec::len)))
    });
    r.step(
        "version",
        || if server == want { Ok(format!("serverInfo {server}")) } else { Err(format!("serverInfo says {server:?}, expected {want}")) },
    );
    let count = |m: &mut Mcp| -> Result<u64, String> {
        let d = m.tool("inspect_drawing", json!({ "entities": false }))?;
        d["entityCount"].as_u64().ok_or_else(|| format!("inspect_drawing has no entityCount: {d}"))
    };
    let mut drawn = 0;
    r.step("draw", || {
        let before = count(&mut m)?;
        m.tool("command_line", json!({ "text": "line 0,0 100,0 100,50" }))?;
        m.tool("command_line", json!({ "text": "" }))?;
        m.tool("execute", json!({ "command": "circle", "params": { "center": [50, 25], "radius": 10 } }))?;
        drawn = count(&mut m)?;
        if drawn >= before + 3 { Ok(format!("{before} → {drawn} objects")) } else { Err(format!("{before} → {drawn} objects (expected 3 more)")) }
    });
    for (ext, name) in [("dwg", "save dwg"), ("dxf", "save dxf"), ("cdy", "save cdy")] {
        let path = out.join(format!("smoke.{ext}"));
        r.step(name, || {
            m.tool("save", json!({ "path": path }))?;
            made(&path, None)
        });
    }
    for (ext, name) in [("dwg", "reopen dwg"), ("dxf", "reopen dxf"), ("cdy", "reopen cdy")] {
        let path = out.join(format!("smoke.{ext}"));
        r.step(name, || {
            m.tool("open", json!({ "path": path }))?;
            let n = count(&mut m)?;
            if n == drawn { Ok(format!("{n} objects")) } else { Err(format!("{n} objects, saved {drawn}")) }
        });
    }
    let pdf = out.join("smoke.pdf");
    r.step("plot pdf", || {
        m.tool("plot", json!({ "path": pdf, "layout": "Model" }))?;
        made(&pdf, Some(b"%PDF"))
    });
    r.skip("walk", "the desktop app's (not in the MCP server)");
    let set = out.join("smoke.sheetset.json");
    let book = out.join("sheets.pdf");
    r.step("sheet set", || {
        let ex = |m: &mut Mcp, p: Value| m.tool("execute", json!({ "command": "sheetset", "params": p }));
        ex(&mut m, json!({ "set": set, "action": "new", "name": "Smoke", "overwrite": true }))?;
        ex(&mut m, json!({ "set": set, "action": "add", "drawing": out.join("smoke.dwg") }))?;
        ex(&mut m, json!({ "set": set, "action": "publish", "path": book }))?;
        made(&book, Some(b"%PDF"))
    });
    r.step("update check", || {
        let latest = latest_release()?;
        if latest == want {
            Ok(format!("the release feed's latest is {latest}: up to date"))
        } else {
            Err(format!("the release feed's latest is {latest}, this is {want}"))
        }
    });
    Ok(())
}

// ---------------------------------------------------------------- the command

pub fn run(root: &Path, rest: &[&str]) -> Result<(), String> {
    let opts = parse(rest)?;
    let want = match &opts.version {
        Some(v) => v.clone(),
        None => latest_release()?,
    };
    let out = opts.out.clone().unwrap_or_else(|| root.join("target").join("smoke"));
    if out.exists() && !opts.keep {
        std::fs::remove_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    }
    std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let out = std::fs::canonicalize(&out).map_err(|e| e.to_string())?;
    println!("smoke test of {} (expecting {want}); output in {}", opts.target.display(), out.display());
    let mut r = Report::default();
    let kind = kind_of(&opts.target);
    let run = match kind {
        Kind::Dmg => {
            let mnt = out.join("mnt");
            std::fs::create_dir_all(&mnt).map_err(|e| e.to_string())?;
            let ok = Command::new("hdiutil")
                .args(["attach", "-nobrowse", "-readonly", "-quiet", "-mountpoint"])
                .arg(&mnt)
                .arg(&opts.target)
                .status()
                .map_err(|e| format!("hdiutil: {e}"))?;
            if !ok.success() {
                return Err(format!("hdiutil attach {} failed", opts.target.display()));
            }
            let _mounted = Mounted(mnt.clone());
            let app = std::fs::read_dir(&mnt)
                .map_err(|e| e.to_string())?
                .flatten()
                .map(|e| e.path())
                .find(|p| p.extension().is_some_and(|x| x == "app"))
                .ok_or("no .app in the disk image")?;
            run_app(&bundle_exe(&app)?, bundle_version(&app).ok(), &want, &out, &mut r)
        }
        Kind::App if opts.target.extension().is_some_and(|x| x == "app") => {
            run_app(&bundle_exe(&opts.target)?, bundle_version(&opts.target).ok(), &want, &out, &mut r)
        }
        Kind::App => run_app(&opts.target, None, &want, &out, &mut r),
        Kind::Mcpb => {
            let dir = out.join("mcpb");
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            // A .mcpb is a zip; tar reads zips on macOS and Windows 10+, unzip elsewhere.
            let ok = Command::new("tar").arg("-xf").arg(&opts.target).arg("-C").arg(&dir).status().is_ok_and(|s| s.success())
                || Command::new("unzip").arg("-oq").arg(&opts.target).arg("-d").arg(&dir).status().is_ok_and(|s| s.success());
            if !ok {
                return Err(format!("couldn't unpack {}", opts.target.display()));
            }
            let exe = dir.join("server").join(if cfg!(windows) { "cadaddy-cli.exe" } else { "cadaddy-cli" });
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755));
            }
            run_cli(&exe, &want, &out, &mut r)
        }
        Kind::Cli => run_cli(&opts.target, &want, &out, &mut r),
    };
    let report = r.json(&opts.target.to_string_lossy(), &want);
    let _ = std::fs::write(out.join("report.json"), serde_json::to_string_pretty(&report).unwrap_or_default());
    run?;
    let failed = r.failed();
    println!("{} steps, {failed} failed; report: {}", r.steps.len(), out.join("report.json").display());
    if failed > 0 { Err(format!("{failed} smoke step(s) failed")) } else { Ok(()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_and_kinds() {
        let o = parse(&["CADaddy-Mac.dmg", "--version", "v0.7.7", "--out", "x"]).unwrap();
        assert_eq!((o.version.as_deref(), o.out, o.keep), (Some("0.7.7"), Some(PathBuf::from("x")), false));
        assert!(parse(&[]).is_err() && parse(&["a", "b"]).is_err() && parse(&["a", "--nope"]).is_err() && parse(&["a", "--version"]).is_err());
        assert_eq!(kind_of(Path::new("/d/CADaddy-Mac.dmg")), Kind::Dmg);
        assert_eq!(kind_of(Path::new("CADaddy.mcpb")), Kind::Mcpb);
        assert_eq!(kind_of(Path::new("server/cadaddy-cli.exe")), Kind::Cli);
        assert_eq!(kind_of(Path::new("/Applications/CADaddy.app")), Kind::App);
    }

    #[test]
    fn plist_versions() {
        let p = "<dict>\n<key>CFBundleName</key>\n<string>CADaddy</string>\n<key>CFBundleShortVersionString</key>\n\t<string>0.7.7</string>\n</dict>";
        assert_eq!(plist_value(p, "CFBundleShortVersionString").as_deref(), Some("0.7.7"));
        assert_eq!(plist_value(p, "Missing"), None);
    }

    #[test]
    fn made_files_are_checked() {
        let dir = std::env::temp_dir().join(format!("cadaddy-smoke-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.pdf");
        std::fs::write(&f, b"%PDF-1.7 x").unwrap();
        assert!(made(&f, Some(b"%PDF")).is_ok());
        assert!(made(&f, Some(b"\x89PNG")).is_err());
        std::fs::write(&f, b"").unwrap();
        assert!(made(&f, None).is_err());
        assert!(made(&dir.join("none"), None).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}
