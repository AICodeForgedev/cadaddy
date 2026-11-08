//! Where MCP tool calls end up: a control-channel method call.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use cadaddy_engine::Session;
use serde_json::{Value, json};

/// Something that answers control-channel methods (`engine.execute`, `cmdline.input`,
/// `drawing.inspect`, `ui.render`…). See `cadaddy_ui_egui::control` for the full list.
pub trait Backend {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, String>;
    /// True when a real UI is attached (`ui.screenshot` works).
    fn has_ui(&self) -> bool;
    fn describe(&self) -> String;
    /// A function that stops what this backend is running, callable from another thread while a
    /// call is blocked (the `cancel` tool and `notifications/cancelled` use it). `None` when the
    /// in-process cancel flag already reaches it.
    fn canceller(&self) -> Option<Box<dyn Fn() + Send>> {
        None
    }
    /// Work that waits for the client to pause (a 3D model the plan's edits left behind is built
    /// then): done when due. Returns how long until it is due when some waits, else `None`.
    fn idle(&mut self) -> Option<Duration> {
        None
    }
}

/// A running CADaddy app reached through its loopback control port.
pub struct Remote {
    addr: String,
    conn: Option<(BufReader<TcpStream>, TcpStream)>,
    next_id: u64,
    /// Open the app when nothing answers (see [`Remote::lazy`]).
    launch: bool,
    /// Which window answered the last hello (title and process id), when the app says.
    window: Option<String>,
    /// How long the hello may take before the window counts as busy ([`HANDSHAKE_WAIT`]).
    hello_wait: Duration,
}

/// How long a launched app has to open its control port.
const LAUNCH_WAIT: Duration = Duration::from_secs(30);
/// Longest a headless call runs a LISP routine typed on the command line before answering (it
/// keeps running at the next call; `cancel` stops it).
const HEADLESS_LISP_LIMIT: Duration = Duration::from_secs(30);
/// How long a window has to answer the hello on a new connection before it counts as busy (its
/// own reply timeout is a minute; one that answers `ping` at all does so at once).
const HANDSHAKE_WAIT: Duration = Duration::from_secs(10);
/// How long a call waits for the window's reply once the hello was answered.
const REPLY_WAIT: Duration = Duration::from_secs(90);
/// How long [`Auto`] answers with the busy error it just got before asking the window again (the
/// server's own follow-up calls after a tool call would each wait the hello out again).
const RETRY_AFTER: Duration = Duration::from_secs(2);

impl Remote {
    pub fn connect(addr: &str) -> std::io::Result<Self> {
        let mut r = Self::lazy(addr, false);
        r.reconnect()?;
        Ok(r)
    }
    /// The app at `addr`, reached at the first call instead of now, so an MCP client can start
    /// the server before the app is open. With `launch`, a call that finds nothing at `addr`
    /// opens the app with that control port first (macOS) and waits for it.
    pub fn lazy(addr: &str, launch: bool) -> Self {
        Self { addr: addr.to_string(), conn: None, next_id: 1, launch, window: None, hello_wait: HANDSHAKE_WAIT }
    }
    /// The same, with another wait for the hello (tests use a short one).
    pub(crate) fn with_hello_wait(mut self, wait: Duration) -> Self {
        self.hello_wait = wait;
        self
    }
    /// Connect, opening the app first when asked to and nothing answers.
    fn reach(&mut self) -> std::io::Result<()> {
        let first = match self.reconnect() {
            Ok(()) => return Ok(()),
            Err(e) => e,
        };
        // A window that refused us or is busy is open: opening another is no answer.
        if matches!(first.kind(), std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::ResourceBusy) {
            return Err(first);
        }
        if !self.launch || !matches!(first.kind(), std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::TimedOut) {
            return Err(not_open(&self.addr, first));
        }
        launch_app(&self.addr)?;
        let start = std::time::Instant::now();
        while start.elapsed() < LAUNCH_WAIT {
            std::thread::sleep(Duration::from_millis(500));
            if self.reconnect().is_ok() {
                return Ok(());
            }
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            format!(
                "CADaddy was opened but its control port {} did not answer; if it was already open without the port, quit it and try again",
                self.addr
            ),
        ))
    }
    fn reconnect(&mut self) -> std::io::Result<()> {
        self.open().map_err(|(e, _)| e)
    }
    /// Connect and say hello. On failure the flag says whether something listens at the address
    /// (it accepted the connection), so a busy or refusing window is never taken for no window.
    fn open(&mut self) -> Result<(), (std::io::Error, bool)> {
        self.conn = None;
        let mut last = std::io::Error::new(std::io::ErrorKind::NotFound, format!("cannot resolve {}", self.addr));
        for sa in self.addr.to_socket_addrs().map_err(|e| (e, false))? {
            match TcpStream::connect_timeout(&sa, Duration::from_millis(800)) {
                Ok(s) => return self.hello(s, sa.port()).map_err(|e| (answered_badly(&self.addr, e), true)),
                Err(e) => last = e,
            }
        }
        Err((last, false))
    }
    /// Authenticate and ask the version on a new connection, waiting at most [`Remote::hello_wait`]
    /// for the window to answer.
    fn hello(&mut self, s: TcpStream, port: u16) -> std::io::Result<()> {
        s.set_nodelay(true).ok();
        s.set_read_timeout(Some(self.hello_wait)).ok();
        let mut reader = BufReader::new(s.try_clone()?);
        let mut s = s;
        authenticate(&mut reader, &mut s, port).map_err(|e| no_answer(e, self.hello_wait))?;
        self.window = version_handshake(&mut reader, &mut s, port).map_err(|e| no_answer(e, self.hello_wait))?;
        s.set_read_timeout(Some(REPLY_WAIT)).ok();
        self.conn = Some((reader, s));
        Ok(())
    }
    /// On failure the flag says whether the request may already have reached the server.
    fn roundtrip(&mut self, line: &str) -> Result<String, (std::io::Error, bool)> {
        if self.conn.is_none() {
            self.reach().map_err(|e| (e, false))?;
        }
        let Some((reader, writer)) = self.conn.as_mut() else { return Err((std::io::Error::other("not connected"), false)) };
        // One write per line (two small ones can wait on a delayed acknowledgement).
        let mut buf = String::with_capacity(line.len() + 1);
        buf.push_str(line);
        buf.push('\n');
        let sent = writer.write_all(buf.as_bytes()).and_then(|()| writer.flush());
        if let Err(e) = sent {
            self.conn = None;
            return Err((e, false));
        }
        let mut reply = String::new();
        match reader.read_line(&mut reply) {
            Ok(0) => {
                self.conn = None;
                Err((std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "connection closed"), true))
            }
            Ok(_) => Ok(reply),
            Err(e) => {
                self.conn = None;
                Err((e, true))
            }
        }
    }
}

/// Present the app's control token, when there is one for `port` (the app writes it each start;
/// `cadaddy_engine::control_auth`). An app that needs none answers `auth` with success too; one
/// from before tokens answers with an error for the unknown method, which is ignored.
fn authenticate(reader: &mut BufReader<TcpStream>, writer: &mut TcpStream, port: u16) -> std::io::Result<()> {
    authenticate_with(reader, writer, cadaddy_engine::control_auth::client_token(port))
}

/// [`authenticate`] with the token given.
pub(crate) fn authenticate_with(reader: &mut BufReader<TcpStream>, writer: &mut TcpStream, token: Option<String>) -> std::io::Result<()> {
    let Some(token) = token else { return Ok(()) };
    let line = format!("{}\n", json!({"id": 0, "method": "auth", "params": {"token": token}}));
    writer.write_all(line.as_bytes())?;
    writer.flush()?;
    let mut reply = String::new();
    reader.read_line(&mut reply)?;
    let v: Value = serde_json::from_str(reply.trim()).unwrap_or(Value::Null);
    if v.get("error").and_then(Value::as_str).is_some_and(|e| e.starts_with("unauthorized")) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "the control token was not accepted (it changes each start: is the token file from another CADaddy?)",
        ));
    }
    Ok(())
}

/// After authenticating, ask the app for its version and tool list; warn on stderr when the
/// bridge (this cadaddy-cli) and the app are different versions so the user knows why a tool
/// might be missing. Non-fatal: a version mismatch doesn't prevent use. A window that doesn't
/// answer (the read times out, or its own reply timeout answers) is busy: an error of kind
/// `ResourceBusy`, never a version warning.
fn version_handshake(reader: &mut BufReader<TcpStream>, writer: &mut TcpStream, port: u16) -> std::io::Result<Option<String>> {
    let bridge_version = env!("CARGO_PKG_VERSION");
    let bridge_exe = std::env::current_exe().unwrap_or_default();
    let req = format!("{}\n", json!({"id": -1, "method": "ping", "params": {"version": true}}));
    if writer.write_all(req.as_bytes()).is_err() || writer.flush().is_err() {
        return Ok(None);
    }
    let mut reply = String::new();
    match reader.read_line(&mut reply) {
        // No answer in time: the read timeout, passed on as a busy window.
        Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => return Err(e),
        Err(_) => return Ok(None),
        Ok(_) => {}
    }
    let v: Value = match serde_json::from_str(reply.trim()) {
        Ok(v) => v,
        Err(_) => return Ok(None),
    };
    if v.get("ok").and_then(Value::as_bool) == Some(false) {
        let e = v.get("error").and_then(Value::as_str).unwrap_or("error");
        // An app that wants a token and was given none answers the first request with "unauthorized"
        // and closes the connection; say so now instead of letting the next call look like a lost reply.
        if e.starts_with("unauthorized") {
            return Err(no_token(port));
        }
        // The window's own reply timeout, a full queue or the rate limit: it is there but not answering.
        return Err(std::io::Error::new(
            std::io::ErrorKind::ResourceBusy,
            format!("its window did not answer (it replied \"{}\")", cadaddy_engine::clip_input(e)),
        ));
    }
    // The app's ping reply may include "version" when the app supports it (older apps just
    // reply "pong"). Extract the version string if present.
    let app_version = v.get("result").and_then(|r| r.as_object()).and_then(|o| o.get("version")).and_then(Value::as_str).or_else(|| {
        // Older apps reply with a simple string "pong" — no version info.
        v.get("result").and_then(Value::as_str).filter(|s| *s != "pong").map(|_| "unknown")
    });
    if let Some(app_ver) = app_version {
        if app_ver != bridge_version {
            eprintln!(
                "warning: version mismatch — cadaddy-cli is {bridge_version} ({}) but the app is {app_ver}; \
                 some tools may be missing or behave differently. Update both to the same version.",
                bridge_exe.display()
            );
        }
    } else if bridge_version != "unknown" {
        // App didn't report a version (old app). Warn that tools might be missing.
        eprintln!(
            "warning: the app at this port did not report its version (cadaddy-cli is {bridge_version}, {}); \
             if tools are missing, update the app.",
            bridge_exe.display()
        );
    }
    Ok(window_description(&v["result"]))
}

/// Which window answered a `ping {version: true}`: its process id and title, and whether it
/// reaches the agent port at all. `None` from an app that doesn't say.
pub(crate) fn window_description(r: &Value) -> Option<String> {
    let pid = r.get("pid").and_then(Value::as_u64)?;
    let title = r.get("title").and_then(Value::as_str).unwrap_or("");
    let held = if r.get("agentPortHeldElsewhere").and_then(Value::as_bool) == Some(true) { "; another window holds the agent port" } else { "" };
    Some(format!("window \"{}\", pid {pid}{held}", cadaddy_engine::clip_input(title)))
}

/// Tell the app at `addr` to stop its render, on a connection of its own (the main one is waiting for
/// the render's reply). Best effort: an app that can't be reached or doesn't know the method has
/// nothing to stop.
fn cancel_remote(addr: &str) {
    let Some(sa) = addr.to_socket_addrs().ok().and_then(|mut a| a.next()) else { return };
    let Ok(s) = TcpStream::connect_timeout(&sa, Duration::from_millis(800)) else { return };
    s.set_nodelay(true).ok();
    s.set_read_timeout(Some(Duration::from_secs(5))).ok();
    let (Ok(read), mut s) = (s.try_clone(), s) else { return };
    let mut reader = BufReader::new(read);
    if authenticate(&mut reader, &mut s, sa.port()).is_err() {
        return;
    }
    let line = format!("{}\n", json!({"id": 1, "method": "control.cancel", "params": {}}));
    if s.write_all(line.as_bytes()).is_ok() && s.flush().is_ok() {
        let mut reply = String::new();
        let _ = reader.read_line(&mut reply);
    }
}

/// Why the app refused for want of a token: no token was found for `port`.
fn no_token(port: u16) -> std::io::Error {
    let looked = cadaddy_engine::control_auth::config_dir()
        .map_or_else(|| "the CADaddy settings folder".to_string(), |d| cadaddy_engine::control_auth::token_path(&d, port).display().to_string());
    std::io::Error::new(std::io::ErrorKind::PermissionDenied, format!("no control token (looked in {looked}; set CADADDY_CONTROL_TOKEN)"))
}

/// A hello read that timed out (`WouldBlock` on Unix, `TimedOut` on Windows) as a busy window.
fn no_answer(e: std::io::Error, waited: Duration) -> std::io::Error {
    if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) {
        std::io::Error::new(
            std::io::ErrorKind::ResourceBusy,
            format!("its window did not answer within {:.0} s (a long command is running, or the window is not responding)", waited.as_secs_f64()),
        )
    } else {
        e
    }
}

/// The error for an app that accepted the connection but would not talk to us, worded with its
/// address: a refused or missing token (`PermissionDenied`), or a busy window (`ResourceBusy`).
/// [`Remote::call`] tells them apart from a lost connection by their kind.
fn answered_badly(addr: &str, e: std::io::Error) -> std::io::Error {
    match e.kind() {
        std::io::ErrorKind::PermissionDenied => std::io::Error::new(e.kind(), format!("CADaddy at {addr} refused the connection: {e}")),
        std::io::ErrorKind::ResourceBusy => std::io::Error::new(
            e.kind(),
            format!(
                "CADaddy at {addr} is busy: {e}. Nothing was sent; try again, or cancel what it runs (the cancel tool, or Esc in the window). This server drives that window and never switches to a hidden session of its own"
            ),
        ),
        _ => e,
    }
}

/// The `window` of a `ui.render` request: two finite corners [[x0, y0], [x1, y1]] around an area
/// (`None` when absent or unusable).
fn render_window(p: &Value) -> Option<cadaddy_engine::geom::Bounds2> {
    let a = p.get("window")?.as_array().filter(|a| a.len() == 2)?;
    let pt = |v: &Value| -> Option<cadaddy_engine::geom::Vec2> {
        let c = v.as_array().filter(|c| c.len() == 2)?;
        let (x, y) = (c[0].as_f64()?, c[1].as_f64()?);
        (x.is_finite() && y.is_finite()).then(|| cadaddy_engine::geom::Vec2::new(x, y))
    };
    let b = cadaddy_engine::geom::Bounds2::new(pt(&a[0])?, pt(&a[1])?);
    (b.width() > 0.0 && b.height() > 0.0).then_some(b)
}

/// The error for an app that doesn't answer: what to do about it.
fn not_open(addr: &str, e: std::io::Error) -> std::io::Error {
    std::io::Error::new(
        e.kind(),
        format!(
            "CADaddy is not answering at {addr} ({e}). Open CADaddy with its control port (`--control PORT`, CADADDY_CONTROL_PORT or CADCRAFT_CONTROL_PORT), or use `mcp --connect {addr}` (--launch is on by default)"
        ),
    )
}

/// Open the app with the control port of `addr` (macOS: `open -a CADaddy`).
fn launch_app(addr: &str) -> std::io::Result<()> {
    let port = addr.rsplit(':').next().filter(|p| p.parse::<u16>().is_ok()).ok_or_else(|| std::io::Error::other(format!("no port in {addr}")))?;
    if cfg!(target_os = "macos") {
        let ok = std::process::Command::new("open").args(["-a", "CADaddy", "--args", "--control", port]).status()?.success();
        if ok { Ok(()) } else { Err(std::io::Error::other("could not open CADaddy (is it in Applications?)")) }
    } else {
        // Windows and Linux: the app installed beside this command-line tool (the installers and
        // the portable folder put them together), started on its own with the control port.
        let name = if cfg!(windows) { "cadaddy.exe" } else { "cadaddy" };
        let app = std::env::current_exe()?.parent().map(|d| d.join(name)).filter(|p| p.is_file());
        let Some(app) = app else {
            return Err(std::io::Error::other(format!(
                "CADaddy is not running and {name} isn't beside this tool; start it with `cadaddy --control {port}`"
            )));
        };
        std::process::Command::new(app)
            .args(["--control", port])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map(|_| ())
    }
}

/// Methods that only read state, so resending them after a lost reply cannot change the drawing twice.
fn is_read_only(method: &str) -> bool {
    matches!(
        method,
        "drawing.inspect"
            | "drawing.summary"
            | "drawing.query"
            | "document.inspect"
            | "engine.commands"
            | "cmdline.state"
            | "control.methods"
            | "ping"
            | "ui.inspect"
            | "app.closePrompt"
            | "ui.render"
            | "ui.screenshot"
    )
}

impl Backend for Remote {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let line = json!({"id": id, "method": method, "params": params}).to_string();
        let reply = match self.roundtrip(&line) {
            Ok(r) => r,
            // Resend only when the request cannot have been applied. After a sent mutation with no reply the outcome is unknown, and
            // resending could apply it twice (for example two circles from one call), so report that instead.
            // An app that refused us (no or wrong control token) said why; that is the answer, not a lost reply.
            // A busy window was sent nothing: the next call tries again.
            Err((e, _)) if matches!(e.kind(), std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::ResourceBusy) => return Err(e.to_string()),
            Err((_, sent)) if !sent || is_read_only(method) => self.roundtrip(&line).map_err(|(e, _)| match e.kind() {
                std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::ResourceBusy => e.to_string(),
                _ => format!("CADaddy at {}: {e}", self.addr),
            })?,
            Err((e, _)) => {
                return Err(format!(
                    "CADaddy at {}: {e}; the reply was lost, so `{method}` may or may not have been applied. Inspect the drawing first",
                    self.addr
                ));
            }
        };
        let v: Value = serde_json::from_str(&reply).map_err(|e| format!("bad reply: {e}"))?;
        if v.get("ok").and_then(Value::as_bool) == Some(true) {
            Ok(v.get("result").cloned().unwrap_or(Value::Null))
        } else {
            let e = v.get("error").and_then(Value::as_str).unwrap_or("error");
            // A timeout says whether the request can still have run (#345).
            Err(match v.get("state").and_then(Value::as_str) {
                Some("not-run") => format!("{e}: CADaddy did not start `{method}` and won't; it is safe to send again"),
                Some("may-have-run") => format!("{e}: `{method}` may or may not have been applied. Inspect the drawing first"),
                _ => e.to_string(),
            })
        }
    }
    fn has_ui(&self) -> bool {
        true
    }
    fn canceller(&self) -> Option<Box<dyn Fn() + Send>> {
        let addr = self.addr.clone();
        Some(Box::new(move || cancel_remote(&addr)))
    }
    fn describe(&self) -> String {
        match &self.window {
            Some(w) => format!("connected to CADaddy at {} ({w})", self.addr),
            None => format!("connected to CADaddy at {}", self.addr),
        }
    }
}

/// What the headless `control.methods` replies: the methods [`Headless`] answers, a subset of the
/// app's (`docs/control-protocol.md`).
const HEADLESS_METHODS: &[(&str, &str, &str)] = &[
    ("cmdline.input", "{text}", "Type a line at the command line exactly like a user (commands, prompts, keywords; empty = Enter)."),
    ("cmdline.script", "{text}", "Run a multi-line script, like a .scr file."),
    ("cmdline.key", "{key: \"enter\"|\"escape\"}", "Press Enter or Escape at the command line."),
    (
        "cmdline.state",
        "",
        "Current prompt (text and bare message), keywords, default, accepted input kinds, busy (a drawing still opening), closePrompt and history tail.",
    ),
    ("engine.execute", "{command, params}", "Run any command with JSON parameters. Never opens a dialog."),
    ("events.subscribe", "{on?}", "Start (or stop) recording events for events.poll."),
    (
        "events.poll",
        "{since?}",
        "Events after `since`: drawing changed (handles), selection, commands, prompt, documents, dialogs; `next` is the next since.",
    ),
    (
        "engine.executeMany",
        "{commands: [{command, params}], stopOnError?}",
        "Run up to 10k commands in one request and one undo step; each result returned (stopOnError: the first failure puts everything back).",
    ),
    (
        "engine.commands",
        "{filter?}",
        "Every command (or those whose id, label or an alias contains `filter`): id, label, menu path, shortcut, aliases, params doc, enabled.",
    ),
    ("drawing.inspect", "{entities?, limit?}", "Drawing summary and entities with handles and geometry."),
    (
        "drawing.summary",
        "{sections?, textOffset?, textLimit?, textContains?}",
        "What the drawing holds, capped and paged: units, extents, layouts, layers and blocks with object counts, all text with positions, title block attributes, xrefs/images/underlays found or missing, dimension styles in use.",
    ),
    (
        "drawing.query",
        "{type?, layer?, space?, window?, crossing?, text?, block?, limit?, offset?}",
        "Objects by type, layer (wildcards), space, region, text or block, as handles (paged).",
    ),
    ("ui.render", "{path?, width?, height?, fit?}", "Render the drawing; replies pngBase64."),
    ("app.open", "{path}", "Open a drawing."),
    (
        "app.save",
        "{path?, force?, version?, binary?}",
        "Save the drawing (Save As with a path); force: true overwrites a file that changed on disk; version/binary choose the DWG/DXF version and binary DXF of a Save As.",
    ),
    ("control.methods", "", "This list."),
    ("ping", "", "Replies \"pong\"."),
];

/// An in-process headless session (no window).
pub struct Headless {
    pub session: Session,
}

impl Default for Headless {
    /// A session with one new drawing whose layouts have their viewport, as the app and NEW start
    /// one (so plotting Layout1 shows the drawing, not a blank sheet).
    fn default() -> Self {
        // As the apps start: with the installed plug-ins.
        let mut session = Session::for_app();
        session.new_drawing_with_viewports(false);
        // No one to ask here: the agent runs the session (dry run and the journal still apply).
        session.agent.confirm = false;
        session.agent.headless = true;
        Headless { session }
    }
}

/// The command-line state, with the same fields as the app's (`cadaddy_ui_egui::control`). There
/// is no command-line widget here: nothing is typed (`buffer`) and the history isn't expanded.
fn state(s: &Session) -> Value {
    let p = s.current_prompt();
    json!({
        "prompt": s.prompt_text(),
        "running": s.running.as_ref().map(|r| r.id.clone()),
        "message": p.as_ref().map(|p| p.message.clone()),
        "keywords": p.as_ref().map(|p| p.keywords.clone()).unwrap_or_default(),
        "default": p.as_ref().and_then(|p| p.default.clone()),
        "accept": p.as_ref().map(|p| p.accept),
        "busy": false,
        "closePrompt": Value::Null,
        "buffer": "",
        "history": s.log.iter().rev().take(20).rev().cloned().collect::<Vec<_>>(),
        "historyExpanded": false,
    })
}

impl Backend for Headless {
    fn idle(&mut self) -> Option<Duration> {
        self.session.update_model_when_idle(cadaddy_engine::cmd::wallgen::IDLE_REBUILD)
    }
    fn call(&mut self, method: &str, p: Value) -> Result<Value, String> {
        let s = &mut self.session;
        let str_p = |k: &str| p.get(k).and_then(Value::as_str).map(str::to_string);
        match method {
            "engine.execute" | "command" => {
                let id = str_p("command").ok_or("missing `command`")?;
                let params = p.get("params").cloned().filter(|v| !v.is_null()).unwrap_or(json!({}));
                s.execute_agent(&id, &params).map_err(|e| e.to_string())
            }
            "engine.executeMany" => {
                let list = p.get("commands").and_then(Value::as_array).ok_or("missing `commands`: [{command, params}]")?;
                let stop = p.get("stopOnError").and_then(Value::as_bool).unwrap_or(false);
                s.execute_many_agent(list, stop).map_err(|e| e.to_string())
            }
            "events.subscribe" => Ok(s.events_subscribe(p.get("on").and_then(Value::as_bool).unwrap_or(true))),
            "events.poll" => Ok(s.events_poll(p.get("since").and_then(Value::as_u64).unwrap_or(0))),
            "cmdline.input" => {
                let before = s.log.len();
                // Typed by an agent: the same rails as its `engine.execute` calls (dry run,
                // what only the user may loosen), as in the app.
                let was = std::mem::replace(&mut s.agent.from_agent, true);
                let r = s.cmdline(&str_p("text").unwrap_or_default());
                s.lisp_finish(HEADLESS_LISP_LIMIT);
                s.agent.from_agent = was;
                // Like the app: a refused line is echoed to the history (and `output`), and also
                // reported in `error`.
                if let Err(e) = &r {
                    s.echo(e.to_string());
                }
                let out: Vec<String> = s.log.iter().skip(before).cloned().collect();
                let mut st = state(s);
                if let Some(o) = st.as_object_mut() {
                    o.insert("output".into(), json!(out));
                    if let Err(e) = r {
                        o.insert("error".into(), json!(e.to_string()));
                    }
                }
                Ok(st)
            }
            "cmdline.script" => {
                let before = s.log.len();
                let was = std::mem::replace(&mut s.agent.from_agent, true);
                let r = s.script(&str_p("text").unwrap_or_default());
                s.lisp_finish(HEADLESS_LISP_LIMIT);
                s.agent.from_agent = was;
                r.map_err(|e| e.to_string())?;
                let out: Vec<String> = s.log.iter().skip(before).cloned().collect();
                Ok(json!({"output": out, "state": state(s)}))
            }
            "cmdline.key" => {
                let before = s.log.len();
                let was = std::mem::replace(&mut s.agent.from_agent, true);
                let r = if str_p("key").is_some_and(|k| k.eq_ignore_ascii_case("escape") || k.eq_ignore_ascii_case("esc")) {
                    s.cancel();
                    Ok(())
                } else {
                    s.input(cadaddy_engine::Input::Enter)
                };
                s.lisp_finish(HEADLESS_LISP_LIMIT);
                s.agent.from_agent = was;
                // Like the app: a refused Enter is echoed to the history (and `output`), and also
                // reported in `error`; the call itself succeeds.
                if let Err(e) = &r {
                    s.echo(e.to_string());
                }
                let out: Vec<String> = s.log.iter().skip(before).cloned().collect();
                let mut st = state(s);
                if let Some(o) = st.as_object_mut() {
                    o.insert("output".into(), json!(out));
                    if let Err(e) = r {
                        o.insert("error".into(), json!(e.to_string()));
                    }
                }
                Ok(st)
            }
            "cmdline.state" => Ok(state(s)),
            "engine.commands" => {
                let f = p.get("filter").and_then(Value::as_str).unwrap_or("");
                Ok(Value::Array(
                    cadaddy_engine::command_specs()
                        .iter()
                        .filter(|c| c.matches(f))
                        .map(|c| serde_json::to_value(c.info(s)).unwrap_or_default())
                        .collect(),
                ))
            }
            "drawing.inspect" | "document.inspect" => s.execute("drawing.inspect", &p).map_err(|e| e.to_string()),
            "drawing.summary" | "drawing.query" => s.execute(method, &p).map_err(|e| e.to_string()),
            "ui.render" => {
                if let Some(path) = str_p("path") {
                    cadaddy_engine::cmd::file::guard_image_output(s, "render", &p, &path, &["png"]).map_err(|e| e.to_string())?;
                }
                let side = |k: &str, d: f64| -> Result<u32, String> {
                    match p.get(k) {
                        None | Some(Value::Null) => Ok(d as u32),
                        Some(v) => v
                            .as_f64()
                            .filter(|x| x.is_finite() && (16.0..=8192.0).contains(x))
                            .map(|x| x as u32)
                            .ok_or_else(|| format!("`{k}` is 16 to 8192 pixels")),
                    }
                };
                let (w, h) = (side("width", 1600.0)?, side("height", 1000.0)?);
                let st = s.state().map_err(|e| e.to_string())?;
                let window = render_window(&p);
                let opts = cadaddy_render::Options { view: window, transparency: s.settings.transparency_display, ..Default::default() };
                let list = cadaddy_render::build(&st.doc, &st.space, &opts);
                let fit = p.get("fit").and_then(Value::as_bool).unwrap_or(true);
                let side = |k: &str| p.get(k).and_then(Value::as_f64).map(|x| x as u32);
                let view = if let Some(win) = &window {
                    cadaddy_render::raster::View::of_window(win, side("width"), side("height"))
                } else if fit {
                    cadaddy_render::raster::View::fit(&list.bounds, w, h, 0.05)
                } else {
                    let v = st.view();
                    cadaddy_render::raster::View { center: v.center, scale: f64::from(h) / v.height.max(1e-12), width: w, height: h }
                };
                let (w, h) = (view.width, view.height);
                let png =
                    cadaddy_render::raster::render_png(&list, &view, &cadaddy_render::raster::RasterOptions::default()).ok_or("render failed")?;
                if let Some(path) = str_p("path") {
                    std::fs::write(&path, &png).map_err(|e| format!("{path}: {e}"))?;
                }
                Ok(json!({"pngBase64": cadaddy_engine::cmd::file::base64_encode(&png), "width": w, "height": h}))
            }
            "app.open" => {
                let path = str_p("path").ok_or("missing path")?;
                s.execute("open", &json!({ "path": path })).map_err(|e| e.to_string())
            }
            "app.save" => {
                let id = if p.get("path").is_some() { "saveas" } else { "qsave" };
                s.execute_agent(id, &p).map_err(|e| e.to_string())
            }
            "ui.screenshot" => Err("no UI in headless mode; use render".into()),
            "control.methods" => Ok(HEADLESS_METHODS
                .iter()
                .map(|(method, params, description)| json!({"method": method, "params": params, "description": description}))
                .collect()),
            "ping" => Ok(json!("pong")),
            other => Err(format!("unknown method `{other}`; `control.methods` lists them (docs/control-protocol.md)")),
        }
    }
    fn has_ui(&self) -> bool {
        false
    }
    fn describe(&self) -> String {
        "headless CADaddy session".into()
    }
}

/// The open CADaddy window when one listens on [`cadaddy_engine::AGENT_PORT`], else an
/// in-process session: decided the first time the server needs to know (listing its tools or
/// its first call), so an app opened before the agent's first request is the one it drives.
/// Only "nothing listens" (the connection is refused or never accepted) picks the session of its
/// own. A window that accepts the connection but doesn't answer (busy, or not responding) is
/// still the window: calls fail with a busy error and the choice is tried again at the next one,
/// so nothing an agent does lands in a session the user can't see.
pub struct Auto {
    addr: String,
    hello_wait: Duration,
    retry_after: Duration,
    chosen: std::cell::RefCell<Option<Box<dyn Backend>>>,
    /// Why the last try could not reach the window that listens (the busy error) and when, until a
    /// choice is made.
    unanswered: std::cell::RefCell<Option<(String, std::time::Instant)>>,
}

impl Default for Auto {
    fn default() -> Self {
        Self::at(&format!("127.0.0.1:{}", cadaddy_engine::AGENT_PORT))
    }
}

impl Auto {
    /// The window listening at `addr` (instead of the agent port), else a session of its own.
    pub fn at(addr: &str) -> Self {
        Self {
            addr: addr.to_string(),
            hello_wait: HANDSHAKE_WAIT,
            retry_after: RETRY_AFTER,
            chosen: Default::default(),
            unanswered: Default::default(),
        }
    }
    /// The same, with other waits for the window's hello and before asking again (tests use short ones).
    #[cfg(test)]
    pub(crate) fn with_waits(mut self, hello: Duration, retry_after: Duration) -> Self {
        self.hello_wait = hello;
        self.retry_after = retry_after;
        self
    }
    /// The busy error got from the window that listens, while it is recent enough to answer with.
    fn recent_unanswered(&self) -> Option<String> {
        self.unanswered.borrow().as_ref().filter(|(_, at)| at.elapsed() < self.retry_after).map(|(e, _)| e.clone())
    }
    /// The window when it answers, a session of its own when nothing listens, else why the window
    /// that listens can't be driven now.
    fn choose(&self) -> Result<Box<dyn Backend>, String> {
        let mut r = Remote::lazy(&self.addr, false).with_hello_wait(self.hello_wait);
        let chosen: Result<Box<dyn Backend>, String> = match r.open() {
            Ok(()) => Ok(Box::new(r)),
            Err((_, false)) => Ok(Box::new(Headless::default())),
            Err((e, true)) => Err(e.to_string()),
        };
        *self.unanswered.borrow_mut() = chosen.as_ref().err().map(|e| (e.clone(), std::time::Instant::now()));
        chosen
    }
    /// The backend, choosing it when not chosen yet; `None` while the window that listens doesn't answer.
    fn chosen(&self) -> Option<std::cell::RefMut<'_, Box<dyn Backend>>> {
        let mut slot = self.chosen.borrow_mut();
        if slot.is_none() {
            *slot = self.choose().ok();
        }
        std::cell::RefMut::filter_map(slot, Option::as_mut).ok()
    }
}

impl Backend for Auto {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        if self.chosen.get_mut().is_none() {
            if let Some(e) = self.recent_unanswered() {
                return Err(e);
            }
            let b = self.choose()?;
            *self.chosen.get_mut() = Some(b);
        }
        match self.chosen.get_mut() {
            Some(b) => b.call(method, params),
            None => Err("no CADaddy backend".into()),
        }
    }
    fn idle(&mut self) -> Option<Duration> {
        self.chosen.get_mut().as_mut().and_then(|b| b.idle())
    }
    /// A window that listens but doesn't answer yet offers the window's tools: it is the one driven.
    fn has_ui(&self) -> bool {
        if self.chosen.borrow().is_none() && self.unanswered.borrow().is_some() {
            return true;
        }
        self.chosen().is_none_or(|b| b.has_ui())
    }
    fn describe(&self) -> String {
        if self.chosen.borrow().is_none()
            && let Some((e, _)) = self.unanswered.borrow().as_ref()
        {
            return format!("the CADaddy window at {} (not answering yet: {e})", self.addr);
        }
        match self.chosen() {
            Some(b) if !b.has_ui() => format!("{} (nothing listens at {}; open CADaddy to drive its window)", b.describe(), self.addr),
            Some(b) => b.describe(),
            None => format!(
                "the CADaddy window at {} (not answering yet: {})",
                self.addr,
                self.unanswered.borrow().as_ref().map_or("busy", |(e, _)| e.as_str())
            ),
        }
    }
    fn canceller(&self) -> Option<Box<dyn Fn() + Send>> {
        match self.chosen.borrow().as_ref() {
            Some(b) => b.canceller(),
            // Not decided yet: a window at the address, if one turns out to be driven, is told too.
            None => {
                let addr = self.addr.clone();
                Some(Box::new(move || cancel_remote(&addr)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    /// A server that reads one request per connection and closes without replying must not see the request twice.
    #[test]
    fn lost_reply_to_a_mutation_is_not_resent() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let server = std::thread::spawn(move || {
            let mut requests = 0;
            listener.set_nonblocking(false).unwrap();
            let (conn, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(conn).read_line(&mut line).unwrap();
            requests += 1;
            // Anything arriving on a second connection would be a resend.
            listener.set_nonblocking(true).unwrap();
            std::thread::sleep(Duration::from_millis(300));
            requests += usize::from(listener.accept().is_ok());
            requests
        });
        let mut remote = Remote::connect(&addr).unwrap();
        let err = remote.call("engine.execute", json!({"command": "circle", "params": {}})).unwrap_err();
        assert!(err.contains("may or may not have been applied"), "{err}");
        assert_eq!(server.join().unwrap(), 1);
    }

    /// The app's timeout reply says whether the request can still run; the error passes that on (#345).
    #[test]
    fn timeout_reply_says_whether_a_retry_is_safe() {
        for (state, want) in [("not-run", "safe to send again"), ("may-have-run", "Inspect the drawing first")] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap().to_string();
            let server = std::thread::spawn(move || {
                let (conn, _) = listener.accept().unwrap();
                let mut rd = BufReader::new(conn.try_clone().unwrap());
                let mut out = conn;
                // The version handshake comes first: answer it like an older app ("pong").
                let mut line = String::new();
                rd.read_line(&mut line).unwrap();
                if serde_json::from_str::<Value>(&line).is_ok_and(|v| v["method"] == "ping") {
                    writeln!(out, "{}", json!({"id": -1, "ok": true, "result": "pong"})).unwrap();
                    line.clear();
                    rd.read_line(&mut line).unwrap();
                }
                writeln!(out, "{}", json!({"id": 1, "ok": false, "error": "timeout", "state": state})).unwrap();
            });
            let mut remote = Remote::connect(&addr).unwrap();
            let err = remote.call("engine.execute", json!({"command": "circle", "params": {}})).unwrap_err();
            assert!(err.starts_with("timeout") && err.contains(want), "{err}");
            server.join().unwrap();
        }
    }

    /// A stand-in window at a free port. `auth` is answered at once (the app's connection thread
    /// does that); everything else waits while `frozen` is set (a window whose UI thread is stuck),
    /// then is answered like a window: with `app_timeout`, after it, the app's own reply timeout,
    /// else nothing until it thaws.
    fn window(app_timeout: Option<Duration>) -> (String, std::sync::Arc<std::sync::atomic::AtomicBool>) {
        use std::sync::atomic::{AtomicBool, Ordering};
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let frozen = std::sync::Arc::new(AtomicBool::new(true));
        let f = frozen.clone();
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(conn) = conn else { return };
                let f = f.clone();
                std::thread::spawn(move || {
                    let mut out = conn.try_clone().unwrap();
                    for line in BufReader::new(conn).lines() {
                        let Ok(req) = serde_json::from_str::<Value>(&line.unwrap_or_default()) else { return };
                        let reply = match req["method"].as_str().unwrap_or("") {
                            "auth" => json!({"ok": true, "result": {"authenticated": true}}),
                            _ if f.load(Ordering::SeqCst) && app_timeout.is_some() => {
                                std::thread::sleep(app_timeout.unwrap());
                                json!({"ok": false, "error": "timeout", "state": "not-run"})
                            }
                            method => {
                                while f.load(Ordering::SeqCst) {
                                    std::thread::sleep(Duration::from_millis(10));
                                }
                                match method {
                                    "ping" => {
                                        json!({"ok": true, "result": {"version": env!("CARGO_PKG_VERSION"), "pid": 7, "title": "Stand-in window"}})
                                    }
                                    _ => json!({"ok": true, "result": {"window": true}}),
                                }
                            }
                        };
                        if writeln!(
                            out,
                            "{}",
                            json!({"id": req["id"], "ok": reply["ok"], "result": reply["result"], "error": reply["error"], "state": reply["state"]})
                        )
                        .is_err()
                        {
                            return;
                        }
                    }
                });
            }
        });
        (addr, frozen)
    }

    /// The reported case: the window on the port runs something long and answers each request
    /// with its own timeout. The server keeps driving that window: a busy error now, the window
    /// once it answers, never a hidden session of its own.
    #[test]
    fn auto_drives_a_busy_window_instead_of_a_hidden_session() {
        let (addr, frozen) = window(Some(Duration::from_millis(50)));
        let mut auto = Auto::at(&addr).with_waits(Duration::from_secs(5), Duration::from_millis(300));
        assert!(auto.has_ui(), "a window listens: its tools are offered");
        let d = auto.describe();
        assert!(d.contains("not answering yet") && d.contains(&addr) && !d.contains("headless"), "{d}");
        let err = auto.call("drawing.inspect", json!({})).unwrap_err();
        assert!(err.contains("is busy") && err.contains("Nothing was sent") && err.contains("timeout"), "{err}");
        frozen.store(false, std::sync::atomic::Ordering::SeqCst);
        // Asked again at once (the server's own follow-up calls): the same answer, without a wait.
        assert_eq!(auto.call("drawing.inspect", json!({})), Err(err));
        std::thread::sleep(Duration::from_millis(400));
        assert_eq!(auto.call("drawing.inspect", json!({})), Ok(json!({"window": true})), "the choice is tried again at the next call");
        assert!(auto.describe().contains("Stand-in window"), "{}", auto.describe());
    }

    /// A window that doesn't answer at all is busy after the hello wait, not after minutes, and is
    /// still the one driven.
    #[test]
    fn a_silent_window_is_busy_within_the_hello_wait() {
        let (addr, frozen) = window(None);
        let mut auto = Auto::at(&addr).with_waits(Duration::from_millis(300), Duration::ZERO);
        let t = std::time::Instant::now();
        let err = auto.call("engine.execute", json!({"command": "circle", "params": {}})).unwrap_err();
        assert!(err.contains("did not answer within") && err.contains("never switches"), "{err}");
        assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
        assert!(auto.has_ui());
        // A window given with --connect says the same.
        let mut remote = Remote::lazy(&addr, false).with_hello_wait(Duration::from_millis(300));
        assert!(remote.call("ping", Value::Null).unwrap_err().contains("is busy"));
        frozen.store(false, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(auto.call("drawing.inspect", json!({})), Ok(json!({"window": true})));
    }

    /// Only nothing listening gives the session of its own, and it says so.
    #[test]
    fn auto_runs_its_own_session_only_when_nothing_listens() {
        let addr = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().to_string();
        let mut auto = Auto::at(&addr);
        assert!(!auto.has_ui());
        let d = auto.describe();
        assert!(d.starts_with("headless CADaddy session") && d.contains("nothing listens"), "{d}");
        assert!(auto.call("drawing.inspect", json!({})).is_ok());
    }
}
