//! Drawings the desktop hands to CADaddy.
//!
//! - The launch arguments: file managers start `cadaddy %F` (the `.desktop` file), some launchers
//!   pass `file://` URIs, and macOS before 10.9 added a `-psn_…` process number. A name that is
//!   not Unicode is passed on (lossily, so its open fails with a message) instead of stopping
//!   the start-up.
//! - The [`Inbox`]: files opened while CADaddy already runs. They go to the running window as new
//!   tabs, as AutoCAD opens a drawing double-clicked in the file manager in the running session.
//!   On Linux and the BSDs a second start with files passes them over a socket in the settings
//!   folder ([`instance`]) and exits; on macOS the open-documents Apple Event feeds the same
//!   inbox (`cadaddy_macos_open`).

use std::ffi::OsStr;
use std::sync::{Arc, Mutex, PoisonError};

/// Files one launch or one hand-over may name; the rest are ignored (a hostile sender can't make
/// the window open thousands of tabs).
pub const MAX_FILES: usize = 256;

/// What the launch arguments ask for.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Launch {
    /// `--control <port>` values, in order (`None`: the flag had no value).
    pub control: Vec<Option<String>>,
    pub sample: bool,
    pub version: bool,
    /// `--new-window`: start a separate window even when one is running.
    pub new_window: bool,
    pub files: Vec<String>,
}

impl Launch {
    /// Parse the arguments after the program name. After `--` every argument is a file.
    pub fn parse<I, S>(args: I) -> Launch
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut launch = Launch::default();
        let mut args = args.into_iter();
        let mut only_files = false;
        while let Some(a) = args.next() {
            let a = a.as_ref();
            if !only_files {
                match a.to_str() {
                    Some("--control") => {
                        launch.control.push(args.next().map(|v| v.as_ref().to_string_lossy().into_owned()));
                        continue;
                    }
                    Some("--sample") => launch.sample = true,
                    Some("--debug") => {}
                    Some("--version" | "-V") => launch.version = true,
                    Some("--new-window") => launch.new_window = true,
                    Some("--") => only_files = true,
                    _ => launch.push_file(a),
                }
                continue;
            }
            launch.push_file(a);
        }
        launch
    }

    fn push_file(&mut self, arg: &OsStr) {
        if self.files.len() < MAX_FILES
            && let Some(f) = file_arg(arg)
        {
            self.files.push(f);
        }
    }

    /// Whether this start hands its files to a running window instead of opening its own:
    /// only a plain "open these files" start (a file manager's). Agents and tests that ask for a
    /// control port or the sample get their own window, as does `--new-window`, and
    /// `CADADDY_SINGLE_INSTANCE=0` turns it off.
    ///
    /// `has_control_port` is true when a control port was resolved from any source (the
    /// `--control` flag or the `CADADDY_CONTROL_PORT` environment variable).
    #[cfg_attr(not(all(unix, not(target_os = "macos"))), allow(dead_code))]
    pub fn hands_over(&self, single_instance_var: Option<&OsStr>, has_control_port: bool) -> bool {
        !self.files.is_empty() && self.joins_running(single_instance_var, has_control_port)
    }

    /// Whether this start takes part in single-window opening at all (it listens for files when it
    /// is the first one).
    ///
    /// `has_control_port` is true when a control port was resolved from any source (the
    /// `--control` flag or the `CADADDY_CONTROL_PORT` environment variable).
    #[cfg_attr(not(all(unix, not(target_os = "macos"))), allow(dead_code))]
    pub fn joins_running(&self, single_instance_var: Option<&OsStr>, has_control_port: bool) -> bool {
        !has_control_port && !self.sample && !self.new_window && single_instance_var.is_none_or(|v| v != "0")
    }
}

/// A launch argument as a file name to open, or `None` for one that names no file.
pub fn file_arg(arg: &OsStr) -> Option<String> {
    let s = arg.to_string_lossy();
    if s.is_empty() || s.starts_with("-psn_") {
        return None;
    }
    if s.get(..7).is_some_and(|p| p.eq_ignore_ascii_case("file://")) {
        return file_uri_path(&s);
    }
    Some(s.into_owned())
}

/// The local path of a `file://` URI (`file:///tmp/a%20b.dxf`, `file://localhost/…`;
/// `file:///C:/x.dxf` on Windows), percent escapes decoded; `None` for another computer's file.
fn file_uri_path(uri: &str) -> Option<String> {
    let rest = uri.get(7..)?;
    let (host, path) = rest.find('/').map_or((rest, ""), |i| (rest.get(..i).unwrap_or(""), rest.get(i..).unwrap_or("")));
    if !(host.is_empty() || host.eq_ignore_ascii_case("localhost")) || path.is_empty() {
        return None;
    }
    // A query or fragment is not part of the name.
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let bytes = percent_decode(path);
    let mut decoded = String::from_utf8_lossy(&bytes).into_owned();
    // `/C:/drawings/a.dxf` is `C:/drawings/a.dxf`.
    if cfg!(windows) && decoded.as_bytes().get(2) == Some(&b':') && decoded.as_bytes().get(1).is_some_and(u8::is_ascii_alphabetic) {
        decoded.remove(0);
    }
    Some(decoded)
}

/// `%XX` escapes decoded; a `%` not followed by two hex digits stays as it is.
fn percent_decode(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while let Some(&c) = b.get(i) {
        let hex = |j: usize| b.get(j).and_then(|h| char::from(*h).to_digit(16));
        if c == b'%'
            && let (Some(hi), Some(lo)) = (hex(i + 1), hex(i + 2))
        {
            out.push(u8::try_from(hi * 16 + lo).unwrap_or(b'?'));
            i += 3;
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

/// Files for the running window, filled from other threads (the hand-over socket, the macOS
/// open-documents event) and taken by the UI thread each frame.
#[derive(Clone, Default)]
pub struct Inbox {
    files: Arc<Mutex<Vec<String>>>,
    /// Woken when files arrive, so an idle window opens them at once. Set once the window exists.
    waker: Arc<Mutex<Option<egui::Context>>>,
}

impl Inbox {
    /// Queue files to open (at most [`MAX_FILES`] waiting) and wake the window.
    #[cfg_attr(windows, allow(dead_code))]
    pub fn push(&self, files: impl IntoIterator<Item = String>) {
        {
            let mut queue = self.files.lock().unwrap_or_else(PoisonError::into_inner);
            for f in files {
                if queue.len() >= MAX_FILES {
                    break;
                }
                if !f.is_empty() {
                    queue.push(f);
                }
            }
        }
        if let Some(ctx) = self.waker.lock().unwrap_or_else(PoisonError::into_inner).as_ref() {
            ctx.request_repaint();
        }
    }

    /// The files waiting, oldest first.
    pub fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.files.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// The window to wake when files arrive.
    #[cfg_attr(not(all(unix, not(target_os = "macos"))), allow(dead_code))]
    pub fn wake(&self, ctx: egui::Context) {
        *self.waker.lock().unwrap_or_else(PoisonError::into_inner) = Some(ctx);
    }
}

/// One running window per settings folder on Linux and the BSDs: the first start listens on
/// `<settings>/instance.sock`; a later start with files sends them there and exits.
///
/// The message is one JSON object, `{"open":["/abs/a.dxf",…]}` (at most [`MAX_MESSAGE`] bytes),
/// answered `ok`. The socket is the user's own (mode 0600, in their settings folder), so it gives
/// nothing a user couldn't do by starting CADaddy with those files.
#[cfg(all(unix, not(target_os = "macos")))]
pub mod instance {
    use super::{Inbox, MAX_FILES};
    use std::io::{Read, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    /// The socket's file name in the settings folder.
    pub const SOCKET: &str = "instance.sock";
    /// Largest message read.
    pub const MAX_MESSAGE: u64 = 1 << 20;
    /// How long either side waits for the other.
    const TIMEOUT: Duration = Duration::from_secs(3);

    /// Hand `files` to the window listening in `dir`. `Ok(true)`: it took them; `Ok(false)`: no
    /// window is listening (or it didn't answer), so this start opens them itself.
    pub fn hand_over(dir: &Path, files: &[String]) -> std::io::Result<bool> {
        let mut stream = match UnixStream::connect(dir.join(SOCKET)) {
            Ok(s) => s,
            Err(_) => return Ok(false),
        };
        stream.set_read_timeout(Some(TIMEOUT))?;
        stream.set_write_timeout(Some(TIMEOUT))?;
        // The running window has its own working folder: names are made absolute here.
        let files: Vec<String> = files.iter().take(MAX_FILES).map(|f| absolute(f)).collect();
        let message = serde_json::json!({ "open": files }).to_string();
        stream.write_all(message.as_bytes())?;
        stream.shutdown(std::net::Shutdown::Write)?;
        let mut reply = String::new();
        stream.take(64).read_to_string(&mut reply)?;
        Ok(reply.trim() == "ok")
    }

    fn absolute(f: &str) -> String {
        std::path::absolute(f).map_or_else(|_| f.to_string(), |p| p.to_string_lossy().into_owned())
    }

    /// The files a hand-over message names (at most [`MAX_FILES`], strings only).
    pub fn parse(message: &[u8]) -> Vec<String> {
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(message) else { return Vec::new() };
        let Some(list) = value.get("open").and_then(serde_json::Value::as_array) else { return Vec::new() };
        list.iter().filter_map(serde_json::Value::as_str).filter(|s| !s.is_empty()).take(MAX_FILES).map(str::to_string).collect()
    }

    /// The listening end; the socket file is removed when it is dropped.
    pub struct Listener {
        path: PathBuf,
    }

    impl Drop for Listener {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    /// Listen in `dir` and put what arrives in `inbox` (a thread serves the socket). `None` when
    /// another window already listens there, or the socket can't be made (the folder is
    /// missing or read-only, or its path is too long for a socket name); this window then just
    /// doesn't take hand-overs.
    pub fn listen(dir: &Path, inbox: Inbox) -> Option<Listener> {
        let path = dir.join(SOCKET);
        let listener = match UnixListener::bind(&path) {
            Ok(l) => l,
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                if UnixStream::connect(&path).is_ok() {
                    log::info!("another CADaddy window takes files opened from the desktop");
                    return None;
                }
                // Left by a window that didn't exit cleanly.
                let _ = std::fs::remove_file(&path);
                match UnixListener::bind(&path) {
                    Ok(l) => l,
                    Err(e) => {
                        log::warn!("files opened from the desktop open new windows: {e}");
                        return None;
                    }
                }
            }
            Err(e) => {
                log::warn!("files opened from the desktop open new windows: {e}");
                return None;
            }
        };
        {
            use std::os::unix::fs::PermissionsExt;
            if let Err(e) = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)) {
                log::warn!("{}: {e}", path.display());
            }
        }
        let spawned = std::thread::Builder::new().name("open-files".into()).spawn(move || {
            for stream in listener.incoming() {
                match stream {
                    Ok(stream) => serve(stream, &inbox),
                    Err(e) => log::warn!("open-files socket: {e}"),
                }
            }
        });
        if let Err(e) = spawned {
            log::warn!("files opened from the desktop open new windows: {e}");
            let _ = std::fs::remove_file(&path);
            return None;
        }
        Some(Listener { path })
    }

    /// Read one hand-over, answer it and queue its files.
    fn serve(mut stream: UnixStream, inbox: &Inbox) {
        let _ = stream.set_read_timeout(Some(TIMEOUT));
        let _ = stream.set_write_timeout(Some(TIMEOUT));
        let mut message = Vec::new();
        if let Err(e) = (&mut stream).take(MAX_MESSAGE).read_to_end(&mut message) {
            log::warn!("open-files socket: {e}");
            return;
        }
        let files = parse(&message);
        let _ = stream.write_all(if files.is_empty() { b"no\n" } else { b"ok\n" });
        if !files.is_empty() {
            log::info!("opening {} file(s) handed over by another start", files.len());
            inbox.push(files);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn launch_arguments_name_files_and_flags() {
        let l = Launch::parse(["--sample", "a.dxf", "--control", "4000", "b c.dwg", "--debug"]);
        assert!(l.sample && !l.version && !l.new_window);
        assert_eq!(l.control, vec![Some("4000".to_string())]);
        assert_eq!(l.files, vec!["a.dxf", "b c.dwg"]);
        assert_eq!(Launch::parse(["--control"]).control, vec![None]);
        assert!(Launch::parse(["-V"]).version);
        // After `--`, a name that looks like a flag is a file.
        assert_eq!(Launch::parse(["--", "--sample", "-V"]).files, vec!["--sample", "-V"]);
    }

    #[test]
    fn finder_and_file_manager_arguments() {
        // macOS (before 10.9) added its process serial number when Finder started an app.
        assert_eq!(Launch::parse(["-psn_0_123456"]).files, Vec::<String>::new());
        // Launchers that pass URIs.
        assert_eq!(file_arg(OsStr::new("file:///tmp/Sample%20Floor%20Plan.dxf")).as_deref(), Some("/tmp/Sample Floor Plan.dxf"));
        assert_eq!(file_arg(OsStr::new("FILE://localhost/tmp/a.dwg")).as_deref(), Some("/tmp/a.dwg"));
        assert_eq!(file_arg(OsStr::new("file:///tmp/%E5%9B%BE.cdy")).as_deref(), Some("/tmp/图.cdy"));
        assert_eq!(file_arg(OsStr::new("file:///tmp/a.dxf#frag")).as_deref(), Some("/tmp/a.dxf"));
        // Another computer's file can't be opened by path.
        assert_eq!(file_arg(OsStr::new("file://server/share/a.dxf")), None);
        assert_eq!(file_arg(OsStr::new("file://")), None);
        // Escapes that aren't escapes stay.
        assert_eq!(file_arg(OsStr::new("file:///tmp/100%.dxf")).as_deref(), Some("/tmp/100%.dxf"));
        assert_eq!(file_arg(OsStr::new("file:///tmp/%zz%4")).as_deref(), Some("/tmp/%zz%4"));
        assert_eq!(file_arg(OsStr::new("")), None);
        assert_eq!(file_arg(OsStr::new("plan.dxf")).as_deref(), Some("plan.dxf"));
    }

    #[cfg(unix)]
    #[test]
    fn a_name_that_is_not_unicode_does_not_stop_the_start() {
        // std::env::args() panics on these; the launch passes them on lossily.
        use std::os::unix::ffi::OsStringExt;
        let l = Launch::parse([OsString::from("--sample"), OsString::from_vec(b"plan\xff.dxf".to_vec())]);
        assert!(l.sample);
        assert_eq!(l.files, vec!["plan\u{fffd}.dxf"]);
    }

    #[test]
    fn a_launch_names_at_most_max_files() {
        let many: Vec<String> = (0..MAX_FILES + 50).map(|i| format!("{i}.dxf")).collect();
        assert_eq!(Launch::parse(&many).files.len(), MAX_FILES);
    }

    #[test]
    fn only_a_plain_open_hands_over() {
        let plain = Launch::parse(["a.dxf"]);
        assert!(plain.hands_over(None, false));
        assert!(plain.hands_over(Some(OsStr::new("1")), false));
        assert!(!plain.hands_over(Some(OsStr::new("0")), false), "CADADDY_SINGLE_INSTANCE=0");
        assert!(!Launch::parse(["--new-window", "a.dxf"]).hands_over(None, false));
        assert!(!Launch::parse(["--control", "4000", "a.dxf"]).hands_over(None, true), "an agent's own window");
        assert!(!Launch::parse(["--sample", "a.dxf"]).hands_over(None, false));
        assert!(!Launch::parse(Vec::<String>::new()).hands_over(None, false), "nothing to hand over");
        assert!(Launch::parse(Vec::<String>::new()).joins_running(None, false), "an empty start still listens");
        assert!(!Launch::parse(Vec::<String>::new()).joins_running(Some(OsStr::new("0")), false));
    }

    /// An agent start whose control port comes from `CADADDY_CONTROL_PORT` (no `--control` flag)
    /// must not hand its files to a running window — it needs its own session.
    #[test]
    fn an_env_var_agent_start_with_files_does_not_hand_over() {
        // Simulates: `CADADDY_CONTROL_PORT=4000 cadaddy plan.dxf` — the env var resolved a port
        // but Launch has no --control flag, so `launch.control` is empty.
        let launch = Launch::parse(["plan.dxf"]);
        assert!(launch.control.is_empty(), "no --control flag");
        assert!(!launch.hands_over(None, true), "env-var agent start must not hand over");
    }

    /// An empty agent start whose control port comes from `CADADDY_CONTROL_PORT` must not join
    /// the single-window scheme (must not listen on instance.sock).
    #[test]
    fn an_env_var_agent_start_without_files_does_not_join() {
        let launch = Launch::parse(Vec::<String>::new());
        assert!(launch.control.is_empty(), "no --control flag");
        assert!(!launch.joins_running(None, true), "env-var agent start must not listen");
    }

    #[test]
    fn the_inbox_keeps_order_and_caps_what_waits() {
        let inbox = Inbox::default();
        inbox.push(["a.dxf".to_string(), String::new(), "b.dwg".to_string()]);
        assert_eq!(inbox.take(), vec!["a.dxf", "b.dwg"]);
        assert!(inbox.take().is_empty());
        inbox.push((0..MAX_FILES * 2).map(|i| i.to_string()));
        assert_eq!(inbox.take().len(), MAX_FILES);
        // A window to wake: pushing requests a repaint.
        let ctx = egui::Context::default();
        inbox.wake(ctx.clone());
        inbox.push(["c.cdy".to_string()]);
        assert!(ctx.has_requested_repaint());
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    mod instance {
        use super::super::instance::{Listener, MAX_MESSAGE, SOCKET, hand_over, listen, parse};
        use super::super::{Inbox, MAX_FILES};
        use std::io::{Read, Write};
        use std::time::{Duration, Instant};

        fn temp_dir(name: &str) -> std::path::PathBuf {
            let dir = std::env::temp_dir().join(format!("cadaddy-open-files-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("a temporary folder");
            dir
        }

        fn wait_for(inbox: &Inbox) -> Vec<String> {
            let start = Instant::now();
            loop {
                let got = inbox.take();
                if !got.is_empty() || start.elapsed() > Duration::from_secs(5) {
                    return got;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }

        #[test]
        fn a_second_start_hands_its_files_to_the_running_window() {
            let dir = temp_dir("handover");
            // No window yet: the start opens the files itself.
            assert!(!hand_over(&dir, &["a.dxf".to_string()]).expect("no error without a window"));
            let inbox = Inbox::default();
            let listener: Listener = listen(&dir, inbox.clone()).expect("the first window listens");
            // A second window doesn't take over the socket.
            assert!(listen(&dir, Inbox::default()).is_none());
            let handed = hand_over(&dir, &["/tmp/Sample Floor Plan.dxf".to_string(), "relative.dwg".to_string()]).expect("handed over");
            assert!(handed);
            let got = wait_for(&inbox);
            assert_eq!(got.first().map(String::as_str), Some("/tmp/Sample Floor Plan.dxf"));
            // A relative name is made absolute against the second start's folder.
            let cwd = std::env::current_dir().expect("a working folder");
            assert_eq!(got.get(1).map(std::path::PathBuf::from), Some(cwd.join("relative.dwg")));
            drop(listener);
            assert!(!dir.join(SOCKET).exists(), "the socket goes with the window");
            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn a_socket_left_by_a_crash_is_replaced() {
            let dir = temp_dir("stale");
            // A socket file nobody listens on (its window crashed).
            drop(std::os::unix::net::UnixListener::bind(dir.join(SOCKET)).expect("a socket"));
            assert!(dir.join(SOCKET).exists());
            assert!(!hand_over(&dir, &["a.dxf".to_string()]).expect("no error"));
            let inbox = Inbox::default();
            let _listener = listen(&dir, inbox.clone()).expect("the stale socket is replaced");
            assert!(hand_over(&dir, &["/tmp/b.dxf".to_string()]).expect("handed over"));
            assert_eq!(wait_for(&inbox), vec!["/tmp/b.dxf"]);
            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn hostile_hand_overs_open_nothing_and_are_capped() {
            assert!(parse(b"").is_empty());
            assert!(parse(b"not json").is_empty());
            assert!(parse(br#"{"open":"a.dxf"}"#).is_empty());
            assert_eq!(parse(br#"{"open":["a.dxf",1,null,"",{"x":1},"b.dxf"]}"#), vec!["a.dxf", "b.dxf"]);
            let many = serde_json::json!({ "open": (0..MAX_FILES * 4).map(|i| i.to_string()).collect::<Vec<_>>() }).to_string();
            assert_eq!(parse(many.as_bytes()).len(), MAX_FILES);
            // Over the socket: garbage is answered "no" and opens nothing; an endless message is
            // cut at MAX_MESSAGE.
            let dir = temp_dir("hostile");
            let inbox = Inbox::default();
            let _listener = listen(&dir, inbox.clone()).expect("listening");
            let mut s = std::os::unix::net::UnixStream::connect(dir.join(SOCKET)).expect("connect");
            s.write_all(b"\xff\xfe garbage").expect("write");
            s.shutdown(std::net::Shutdown::Write).expect("shutdown");
            let mut reply = String::new();
            s.read_to_string(&mut reply).expect("reply");
            assert_eq!(reply.trim(), "no");
            let mut s = std::os::unix::net::UnixStream::connect(dir.join(SOCKET)).expect("connect");
            let chunk = vec![b' '; 64 * 1024];
            let mut sent = 0u64;
            while sent <= MAX_MESSAGE {
                if s.write_all(&chunk).is_err() {
                    break;
                }
                sent += chunk.len() as u64;
            }
            let _ = s.shutdown(std::net::Shutdown::Write);
            let mut reply = String::new();
            let _ = s.read_to_string(&mut reply);
            assert_eq!(reply.trim(), "no");
            assert!(inbox.take().is_empty());
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}
