//! Loopback JSON-lines control server: one request per line, one reply per line.
//! This is the transport the MCP server (`cadaddy-cli mcp --connect`) wraps.
//!
//! The server only reads requests: a line that is not a JSON object with a string `method` (an
//! HTTP request line, a stray `nc`, binary data) gets one error reply and the connection is
//! closed, so nothing sent after it runs. Every resource a client can hold is bounded
//! ([`Limits`]): simultaneous connections, request line length (much shorter until the client
//! has authenticated), queued requests, requests per second, and how long a socket may sit idle
//! or blocked on a write.
//!
//! With a token (the default; `cadaddy_engine::control_auth`), a connection's first request must
//! be `{"method": "auth", "params": {"token": …}}` or carry a top-level `"token"`; anything else
//! is refused, logged, and the connection closed. The token is random per start and written to a
//! file only the user can read, so other users' programs can't drive the app.

use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::time::{Duration, Instant};

use cadaddy_ui_egui::ControlRequest;
use serde_json::{Value, json};

/// Resource bounds for the control server.
#[derive(Clone, Debug)]
struct Limits {
    /// The token clients must present first (`None`: none needed).
    token: Option<Arc<str>>,
    /// Connections served at once; more get an error line and are closed.
    max_clients: usize,
    /// Longest request line in bytes (newline excluded); a longer one closes the connection.
    max_line: usize,
    /// Requests waiting for the app at once; more are answered `busy`.
    queue: usize,
    /// How long a connection may go without sending a byte before it is closed.
    idle_timeout: Duration,
    /// How long a reply write may block before the connection is closed.
    write_timeout: Duration,
    /// How long a request waits for the app's answer.
    reply_timeout: Duration,
    /// Longest line before the connection has authenticated.
    pre_auth_line: usize,
    /// Requests a connection may send per second on average, and in one burst; more are
    /// answered `rate limited` (not run) without closing the connection.
    rate: f64,
    burst: f64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            token: None,
            max_clients: 16,
            // Generous, so requests that carry whole drawings or images as base64 still fit.
            max_line: 256 * 1024 * 1024,
            queue: 64,
            idle_timeout: Duration::from_secs(600),
            write_timeout: Duration::from_secs(30),
            reply_timeout: Duration::from_secs(60),
            pre_auth_line: 64 * 1024,
            rate: 500.0,
            burst: 2000.0,
        }
    }
}

/// A new random token (32 hex digits); `None` when the system has no randomness to give.
pub fn new_token() -> Option<String> {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).ok()?;
    Some(b.iter().map(|x| format!("{x:02x}")).collect())
}

/// Listen on every port of `ports` (127.0.0.1 only), all feeding one stream of requests: an
/// explicit `--control` port and the agent port together. A port that can't be bound (another
/// window has it) is logged and skipped. With `token`, clients must present it; it is written
/// for each port bound to its file under the settings folder `config`.
pub fn start_ports(ports: &[u16], ctx: egui::Context, token: Option<String>, config: Option<&std::path::Path>) -> Receiver<ControlRequest> {
    let token: Option<Arc<str>> = token.map(Arc::from);
    let (tx, rx) = sync_channel::<ControlRequest>(Limits::default().queue.max(1));
    let mut seen = Vec::new();
    for &port in ports {
        if seen.contains(&port) {
            continue;
        }
        seen.push(port);
        match TcpListener::bind(("127.0.0.1", port)) {
            Ok(l) => {
                log::info!("control server listening on 127.0.0.1:{port}{}", if token.is_some() { " (token required)" } else { "" });
                if let (Some(t), Some(dir)) = (&token, config) {
                    let path = cadaddy_engine::control_auth::token_path(dir, port);
                    if let Err(e) = cadaddy_engine::control_auth::write_token(&path, t) {
                        log::error!("control server: couldn't write the token to {}: {e} (clients can set CADADDY_CONTROL_TOKEN)", path.display());
                    }
                }
                let limits = Limits { token: token.clone(), ..Limits::default() };
                let (tx, ctx) = (tx.clone(), ctx.clone());
                std::thread::spawn(move || accept_loop(l, tx, ctx, limits));
            }
            Err(e) => log::error!("control server failed to bind 127.0.0.1:{port}: {e}"),
        }
    }
    rx
}

/// Accept connections on `listener` under `limits`; requests arrive on the returned receiver.
#[cfg(test)]
fn serve_listener(listener: TcpListener, limits: Limits, ctx: egui::Context) -> Receiver<ControlRequest> {
    let (tx, rx) = sync_channel::<ControlRequest>(limits.queue.max(1));
    std::thread::spawn(move || accept_loop(listener, tx, ctx, limits));
    rx
}

/// A connection's place among [`Limits::max_clients`], given back when the connection ends (also
/// when its thread can't be started, or panics).
struct Slot(Arc<AtomicUsize>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

fn accept_loop(listener: TcpListener, tx: SyncSender<ControlRequest>, ctx: egui::Context, limits: Limits) {
    let open = Arc::new(AtomicUsize::new(0));
    for mut stream in listener.incoming().flatten() {
        let _ = stream.set_write_timeout(Some(limits.write_timeout));
        if open.fetch_add(1, Ordering::SeqCst) >= limits.max_clients {
            open.fetch_sub(1, Ordering::SeqCst);
            log::warn!("control server: refused a connection from {}: too many connections", peer(&stream));
            let _ = writeln!(stream, "{}", json!({"ok": false, "error": "too many control connections"}));
            continue;
        }
        let slot = Slot(Arc::clone(&open));
        let tx = tx.clone();
        let ctx = ctx.clone();
        let limits = limits.clone();
        let _ = std::thread::Builder::new().spawn(move || {
            let _slot = slot;
            serve(stream, &tx, &ctx, &limits);
        });
    }
}

/// Read one `\n`-terminated line of at most `max` bytes into `buf` (newline stripped).
/// `Ok(false)` at end of stream; `Err` when the line is too long or the read fails.
fn read_bounded_line(reader: &mut impl BufRead, buf: &mut Vec<u8>, max: usize) -> Result<bool, String> {
    buf.clear();
    // `take` bounds the bytes buffered for one line: a peer can't make us allocate more than `max`.
    let limit = u64::try_from(max).unwrap_or(u64::MAX).saturating_add(1);
    let n = reader.take(limit).read_until(b'\n', buf).map_err(|e| match e.kind() {
        // The idle timeout ran out (Windows reports it as TimedOut, Unix as WouldBlock).
        ErrorKind::TimedOut | ErrorKind::WouldBlock => "request line not completed in time; closing the connection".to_string(),
        _ => e.to_string(),
    })?;
    if n == 0 {
        return Ok(false);
    }
    if buf.last() == Some(&b'\n') {
        buf.pop();
        if buf.last() == Some(&b'\r') {
            buf.pop();
        }
        return Ok(true);
    }
    if buf.len() > max {
        return Err(format!("request line longer than {max} bytes; closing the connection"));
    }
    // Last line without a newline, then end of stream.
    Ok(true)
}

/// One request line, parsed.
enum Line {
    Request {
        id: Value,
        method: String,
        params: Value,
        /// A `token` sent with the request itself.
        token: Option<String>,
    },
    Blank,
    /// Not a request: reply with this error and close the connection.
    Reject(String),
}

fn parse_line(bytes: &[u8]) -> Line {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Line::Reject("not a control request (invalid UTF-8); closing the connection".into());
    };
    let text = text.trim();
    if text.is_empty() {
        return Line::Blank;
    }
    if is_http_request_line(text) {
        return Line::Reject("HTTP is not served here: this port takes JSON lines; closing the connection".into());
    }
    let msg = match serde_json::from_str::<Value>(text) {
        Ok(v) => v,
        Err(e) => return Line::Reject(format!("bad JSON ({e}); expected one JSON request object per line; closing the connection")),
    };
    let Some(obj) = msg.as_object() else {
        return Line::Reject("not a control request (expected a JSON object); closing the connection".into());
    };
    let Some(method) = obj.get("method").and_then(Value::as_str) else {
        return Line::Reject("not a control request (missing string `method`); closing the connection".into());
    };
    Line::Request {
        id: obj.get("id").cloned().unwrap_or(Value::Null),
        method: method.to_string(),
        params: obj.get("params").cloned().unwrap_or(json!({})),
        token: obj.get("token").and_then(Value::as_str).map(str::to_string),
    }
}

fn peer(s: &TcpStream) -> String {
    s.peer_addr().map_or_else(|_| "an unknown address".into(), |a| a.to_string())
}

/// Requests a connection may send now ([`Limits::rate`], [`Limits::burst`]).
struct Bucket {
    level: f64,
    at: Instant,
}

impl Bucket {
    fn new(limits: &Limits) -> Self {
        Self { level: limits.burst, at: Instant::now() }
    }
    /// Take one request's allowance; `false` when the connection is over its rate.
    fn take(&mut self, limits: &Limits) -> bool {
        let now = Instant::now();
        self.level = (self.level + now.duration_since(self.at).as_secs_f64() * limits.rate).min(limits.burst);
        self.at = now;
        if self.level >= 1.0 {
            self.level -= 1.0;
            true
        } else {
            false
        }
    }
}

/// An HTTP/1 request line (`POST / HTTP/1.1`) or the HTTP/2 preface (`PRI * HTTP/2.0`).
fn is_http_request_line(text: &str) -> bool {
    const METHODS: [&str; 10] = ["GET", "HEAD", "POST", "PUT", "DELETE", "CONNECT", "OPTIONS", "TRACE", "PATCH", "PRI"];
    text.split_once(' ').is_some_and(|(method, _)| METHODS.contains(&method))
        || text.rsplit_once(' ').is_some_and(|(_, version)| version.starts_with("HTTP/"))
}

/// A reply line in one write. `writeln!` wrote the JSON and the newline separately, and with
/// Nagle's algorithm the second waited for the client's delayed acknowledgement: about 0.3 s on
/// one call in every few on Windows (UI review 0.7.7c/e: the "stalls" agents measured).
fn send_line(out: &mut impl Write, line: &str) -> std::io::Result<()> {
    let mut buf = String::with_capacity(line.len() + 1);
    buf.push_str(line);
    buf.push('\n');
    out.write_all(buf.as_bytes())?;
    out.flush()
}

fn serve(stream: TcpStream, tx: &SyncSender<ControlRequest>, ctx: &egui::Context, limits: &Limits) {
    // Replies go out at once instead of waiting to be combined with more (Nagle).
    let _ = stream.set_nodelay(true);
    if stream.set_read_timeout(Some(limits.idle_timeout)).is_err() || stream.set_write_timeout(Some(limits.write_timeout)).is_err() {
        return;
    }
    let Ok(read) = stream.try_clone() else { return };
    let mut out = stream;
    let mut reader = BufReader::new(read);
    let mut buf = Vec::new();
    let who = peer(&out);
    let mut authed = limits.token.is_none();
    let mut bucket = Bucket::new(limits);
    loop {
        let max = if authed { limits.max_line } else { limits.pre_auth_line };
        let reject = match read_bounded_line(&mut reader, &mut buf, max) {
            Ok(false) => break,
            // Nothing of a line arrived (idle timeout, or the peer is gone): close without a word.
            // A line nobody asked for would be read as the reply to the client's next request.
            Err(_) if buf.is_empty() => break,
            Err(e) => e,
            Ok(true) => match parse_line(&buf) {
                Line::Blank => continue,
                Line::Reject(e) => e,
                Line::Request { id, method, params, token } => {
                    if let Some(want) = &limits.token
                        && !authed
                    {
                        let given = if method == "auth" { params.get("token").and_then(Value::as_str).map(str::to_string) } else { token };
                        if !given.is_some_and(|g| cadaddy_engine::control_auth::same(&g, want)) {
                            log::warn!("control server: refused {who}: no valid token");
                            let _ = send_line(
                                &mut out,
                                &json!({"id": id, "ok": false, "error": "unauthorized: send {\"method\": \"auth\", \"params\": {\"token\": …}} first; the token is in the CADaddy settings folder, control/<port>.token (or CADADDY_CONTROL_TOKEN)"}).to_string(),
                            );
                            break;
                        }
                        authed = true;
                    }
                    if method == "auth" {
                        authed = true;
                        if send_line(&mut out, &json!({"id": id, "ok": true, "result": {"authenticated": true}}).to_string()).is_err() {
                            break;
                        }
                        continue;
                    }
                    if !bucket.take(limits) {
                        let r = json!({"id": id, "ok": false, "error": "rate limited: too many requests; slow down", "state": "not-run"});
                        if send_line(&mut out, &r.to_string()).is_err() {
                            break;
                        }
                        continue;
                    }
                    let (req, pending) = ControlRequest::new(method, params);
                    let mut r = match tx.try_send(req) {
                        Ok(()) => {
                            ctx.request_repaint();
                            // On timeout the request is cancelled if the app hasn't started it, and
                            // the reply's `state` says whether it can still have run.
                            pending.wait_nudging(limits.reply_timeout, || ctx.request_repaint())
                        }
                        Err(TrySendError::Full(_)) => json!({"ok": false, "error": "busy: too many queued requests", "state": "not-run"}),
                        Err(TrySendError::Disconnected(_)) => break,
                    };
                    if let Some(o) = r.as_object_mut() {
                        o.insert("id".into(), id);
                    }
                    if send_line(&mut out, &r.to_string()).is_err() {
                        break;
                    }
                    continue;
                }
            },
        };
        // The connection closes either way; a peer that can't read the reason loses nothing.
        log::warn!("control server: closed {who}: {reject}");
        let _ = send_line(&mut out, &json!({"ok": false, "error": reject}).to_string());
        break;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_are_read_up_to_the_limit() {
        let mut buf = Vec::new();
        let mut input: &[u8] = b"aaaaaaaaaa\nbb\r\ncc";
        assert_eq!(read_bounded_line(&mut input, &mut buf, 10), Ok(true));
        assert_eq!(buf, b"aaaaaaaaaa", "a line exactly at the limit is accepted");
        assert_eq!(read_bounded_line(&mut input, &mut buf, 10), Ok(true));
        assert_eq!(buf, b"bb", "CRLF is stripped");
        assert_eq!(read_bounded_line(&mut input, &mut buf, 10), Ok(true));
        assert_eq!(buf, b"cc", "a last line without a newline is still read");
        assert_eq!(read_bounded_line(&mut input, &mut buf, 10), Ok(false), "then end of stream");
    }

    #[test]
    fn an_overlong_line_is_refused_without_reading_the_rest() {
        let mut buf = Vec::new();
        let long = [b'x'; 100];
        let mut input: &[u8] = &long;
        assert!(read_bounded_line(&mut input, &mut buf, 10).is_err());
        assert!(buf.len() <= 11, "at most limit + 1 bytes are buffered, got {}", buf.len());
        assert_eq!(input.len(), 100 - 11, "the rest of the line is left unread");
    }

    fn short() -> Limits {
        Limits {
            queue: 1,
            idle_timeout: Duration::from_millis(300),
            write_timeout: Duration::from_millis(300),
            reply_timeout: Duration::from_millis(300),
            ..Limits::default()
        }
    }

    /// The control server on a loopback port under `limits`; requests arrive on the receiver.
    fn serve_with(limits: Limits) -> (std::net::SocketAddr, Receiver<ControlRequest>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        (addr, serve_listener(listener, limits, egui::Context::default()))
    }

    /// The control server on a loopback port, with a stand-in UI that echoes each method.
    fn serve_echo() -> std::net::SocketAddr {
        let (addr, rx) = serve_with(Limits::default());
        std::thread::spawn(move || {
            for req in rx {
                let _ = req.reply.send(json!({"ok": true, "method": req.method}));
            }
        });
        addr
    }

    /// Send `input` on a new connection and read reply lines until the server closes it.
    fn exchange(addr: std::net::SocketAddr, input: &[u8]) -> Vec<Value> {
        let mut c = TcpStream::connect(addr).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        c.write_all(input).unwrap();
        let mut replies = Vec::new();
        for line in BufReader::new(c).lines() {
            // A read timeout here means the server kept the connection open.
            let Some(line) = closed_or(line) else { break };
            replies.push(serde_json::from_str(&line).unwrap());
        }
        replies
    }

    /// `r`, or `None` when the server reset the connection: Windows aborts a connection whose
    /// read ran into its timeout (the idle close), where Unix ends it cleanly.
    fn closed_or<T>(r: std::io::Result<T>) -> Option<T> {
        match r {
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => None,
            r => Some(r.unwrap()),
        }
    }

    /// The echo server under `limits`.
    fn echo_with(limits: Limits) -> (std::net::SocketAddr, std::sync::mpsc::Receiver<String>) {
        let (addr, rx) = serve_with(limits);
        let (fwd, seen) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for req in rx {
                let _ = fwd.send(req.method.clone());
                let _ = req.reply.send(json!({"ok": true, "method": req.method}));
            }
        });
        (addr, seen)
    }

    /// Send `input`, close our side, and read every reply until the server closes too.
    fn send_all(addr: std::net::SocketAddr, input: &[u8]) -> Vec<Value> {
        let mut c = TcpStream::connect(addr).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        c.write_all(input).unwrap();
        c.shutdown(std::net::Shutdown::Write).unwrap();
        BufReader::new(c).lines().map(|l| serde_json::from_str(&l.unwrap()).unwrap()).collect()
    }

    fn with_token() -> Limits {
        Limits { token: Some(Arc::from("s3cret")), ..short() }
    }

    #[test]
    fn without_the_token_nothing_runs_and_the_connection_closes() {
        let (addr, seen) = echo_with(with_token());
        for first in [
            &b"{\"id\":1,\"method\":\"cmdline.input\",\"params\":{\"text\":\"erase all\"}}\n{\"id\":2,\"method\":\"ping\"}\n"[..],
            b"{\"id\":1,\"method\":\"auth\",\"params\":{\"token\":\"wrong\"}}\n{\"id\":2,\"method\":\"ping\"}\n",
            b"{\"id\":1,\"method\":\"ping\",\"token\":\"s3cre\"}\n",
            b"{\"id\":1,\"method\":\"auth\"}\n",
        ] {
            let r = send_all(addr, first);
            assert_eq!(r.len(), 1, "{r:?}");
            assert!(r[0]["error"].as_str().unwrap().starts_with("unauthorized"), "{r:?}");
            assert_eq!(r[0]["id"], 1);
        }
        assert!(seen.recv_timeout(Duration::from_millis(200)).is_err(), "nothing reached the app");
    }

    #[test]
    fn the_token_by_auth_or_on_the_first_request_opens_the_connection() {
        let (addr, seen) = echo_with(with_token());
        let r = send_all(addr, b"{\"id\":1,\"method\":\"auth\",\"params\":{\"token\":\"s3cret\"}}\n{\"id\":2,\"method\":\"ping\"}\n");
        assert_eq!(r.len(), 2, "{r:?}");
        assert_eq!(r[0]["result"]["authenticated"], true);
        assert_eq!((r[1]["id"].clone(), r[1]["method"].clone()), (json!(2), json!("ping")));
        let r = send_all(addr, b"{\"id\":7,\"method\":\"ping\",\"token\":\"s3cret\"}\n{\"id\":8,\"method\":\"ping\"}\n");
        assert_eq!(r.len(), 2, "{r:?}");
        assert!(r.iter().all(|x| x["ok"] == true), "{r:?}");
        assert_eq!(seen.iter().take(3).count(), 3);
    }

    #[test]
    fn before_the_token_only_short_lines_are_read() {
        let (addr, _seen) = echo_with(Limits { pre_auth_line: 100, ..with_token() });
        let long = format!("{{\"id\":1,\"method\":\"auth\",\"params\":{{\"token\":\"{}\"}}}}\n", "x".repeat(200));
        let r = send_all(addr, long.as_bytes());
        assert!(r[0]["error"].as_str().unwrap().contains("longer than 100"), "{r:?}");
        // After it, the usual limit.
        let ok = format!(
            "{{\"id\":1,\"method\":\"auth\",\"params\":{{\"token\":\"s3cret\"}}}}\n{{\"id\":2,\"method\":\"x\",\"params\":{{\"t\":\"{}\"}}}}\n",
            "y".repeat(500)
        );
        let r = send_all(addr, ok.as_bytes());
        assert_eq!(r.len(), 2, "{r:?}");
        assert_eq!(r[1]["ok"], true);
    }

    #[test]
    fn a_flood_of_requests_is_rate_limited_without_closing() {
        let (addr, _seen) = echo_with(Limits { rate: 0.5, burst: 2.0, ..short() });
        let r = send_all(addr, b"{\"id\":1,\"method\":\"a\"}\n{\"id\":2,\"method\":\"b\"}\n{\"id\":3,\"method\":\"c\"}\n");
        assert_eq!(r.len(), 3, "{r:?}");
        assert_eq!((r[0]["ok"].clone(), r[1]["ok"].clone()), (json!(true), json!(true)));
        assert!(r[2]["error"].as_str().unwrap().starts_with("rate limited"), "{r:?}");
        assert_eq!(r[2]["state"], "not-run");
    }

    #[test]
    fn without_a_token_auth_still_answers() {
        let (addr, _seen) = echo_with(short());
        let r = send_all(addr, b"{\"id\":1,\"method\":\"auth\",\"params\":{\"token\":\"anything\"}}\n{\"id\":2,\"method\":\"ping\"}\n");
        assert_eq!(r.len(), 2, "{r:?}");
        assert!(r.iter().all(|x| x["ok"] == true));
    }

    /// A port nothing listens on now.
    fn free_port() -> u16 {
        TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap().port()
    }

    /// The first line the server answers to `line` on `port`.
    fn first_reply(port: u16, line: &str) -> Value {
        let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        writeln!(c, "{line}").unwrap();
        let mut reply = String::new();
        BufReader::new(c).read_line(&mut reply).unwrap();
        serde_json::from_str(&reply).unwrap()
    }

    #[test]
    fn a_token_from_an_earlier_start_is_refused() {
        // Two starts as the app does them (each with a fresh token written to the settings
        // folder): the first start's token doesn't open the second's port.
        let dir = std::env::temp_dir().join(format!("cadaddy-oldtoken-{}", std::process::id()));
        let (p1, p2) = (free_port(), free_port());
        let _rx1 = start_ports(&[p1], egui::Context::default(), new_token(), Some(&dir));
        let _rx2 = start_ports(&[p2], egui::Context::default(), new_token(), Some(&dir));
        let read = |p: u16| std::fs::read_to_string(cadaddy_engine::control_auth::token_path(&dir, p)).unwrap().trim().to_string();
        let (old, new) = (read(p1), read(p2));
        assert_ne!(old, new);
        let r = first_reply(p2, &json!({ "id": 1, "method": "auth", "params": { "token": old } }).to_string());
        assert!(r["error"].as_str().is_some_and(|e| e.starts_with("unauthorized")), "{r}");
        let r = first_reply(p2, &json!({ "id": 2, "method": "auth", "params": { "token": new } }).to_string());
        assert_eq!(r["result"]["authenticated"], true, "{r}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_port_is_not_reachable_from_another_computer() {
        // Listening on 127.0.0.1 only: this computer's own network address (what another computer
        // would connect to) gets no answer.
        let port = free_port();
        let _rx = start_ports(&[port], egui::Context::default(), new_token(), None);
        assert!(TcpStream::connect(("127.0.0.1", port)).is_ok());
        // The address this computer would send from; no packet is sent.
        let lan = std::net::UdpSocket::bind(("0.0.0.0", 0)).and_then(|u| u.connect(("192.0.2.1", 9)).and_then(|()| u.local_addr()));
        match lan.map(|a| a.ip()) {
            Ok(ip) if !ip.is_loopback() && !ip.is_unspecified() => {
                let r = TcpStream::connect_timeout(&std::net::SocketAddr::new(ip, port), Duration::from_secs(2));
                assert!(r.is_err(), "the control port answered on {ip}");
            }
            // No network here: nothing else could reach it either.
            _ => {}
        }
    }

    #[test]
    fn tokens_are_random() {
        let (a, b) = (new_token().unwrap(), new_token().unwrap());
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
    }

    /// A browser can POST to the port: its request line closes the connection before the JSON
    /// line in its body is read, so nothing is dispatched.
    #[test]
    fn an_http_request_closes_the_connection_before_its_body_is_dispatched() {
        let (addr, rx) = serve_with(short());
        let http = b"POST / HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: text/plain\r\n\r\n\
            {\"id\":1,\"method\":\"cmdline.input\",\"params\":{\"text\":\"circle 0,0 5\"}}\n";
        let replies = exchange(addr, http);
        assert_eq!(replies.len(), 1, "{replies:?}");
        assert_eq!(replies[0]["ok"], false);
        assert!(replies[0]["error"].as_str().unwrap().contains("closing the connection"), "{replies:?}");
        assert!(rx.try_recv().is_err(), "the body was dispatched");
    }

    #[test]
    fn a_line_that_is_not_a_request_closes_the_connection() {
        for junk in [&b"hello\n"[..], b"[1,2]\n", b"{\"id\":1}\n", b"{\"method\":5}\n", b"\xff\xfe\n", b"GET /x HTTP/1.1\n"] {
            let addr = serve_echo();
            let mut input = junk.to_vec();
            input.extend_from_slice(b"{\"id\":2,\"method\":\"ping\"}\n");
            let replies = exchange(addr, &input);
            assert_eq!(replies.len(), 1, "{replies:?}");
            assert_eq!(replies[0]["ok"], false);
        }
        assert!(is_http_request_line("OPTIONS * HTTP/1.1"));
        assert!(is_http_request_line("PRI * HTTP/2.0"));
        assert!(!is_http_request_line("{\"method\":\"GET /\"}"));
    }

    #[test]
    fn an_idle_connection_is_closed() {
        let (addr, _rx) = serve_with(short());
        let mut c = TcpStream::connect(addr).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let t = std::time::Instant::now();
        let mut got = Vec::new();
        closed_or(c.read_to_end(&mut got));
        assert!(got.is_empty(), "{}", String::from_utf8_lossy(&got));
        assert!(t.elapsed() < Duration::from_secs(4));
    }

    #[test]
    fn a_full_queue_answers_busy() {
        let (addr, rx) = serve_with(short());
        // No UI drains the one-place queue: the first request fills it, the second finds it full.
        let first = std::thread::spawn(move || exchange(addr, b"{\"id\":1,\"method\":\"m\"}\n"));
        let deadline = std::time::Instant::now() + Duration::from_secs(4);
        let busy = loop {
            let mut c = TcpStream::connect(addr).unwrap();
            c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            c.write_all(b"{\"id\":2,\"method\":\"m\"}\n").unwrap();
            let mut line = String::new();
            BufReader::new(c).read_line(&mut line).unwrap();
            let v: Value = serde_json::from_str(&line).unwrap();
            if v["error"].as_str().is_some_and(|e| e.starts_with("busy")) || std::time::Instant::now() > deadline {
                break v;
            }
        };
        assert_eq!((busy["id"].as_i64(), busy["state"].as_str()), (Some(2), Some("not-run")), "{busy}");
        assert_eq!(first.join().unwrap()[0]["state"], "not-run");
        drop(rx);
    }

    #[test]
    fn requests_are_answered_with_their_ids() {
        let addr = serve_echo();
        let mut c = TcpStream::connect(addr).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        c.write_all(b"{\"id\":3,\"method\":\"ping\"}\r\n\n{\"id\":4,\"method\":\"pong\"}\n").unwrap();
        let mut r = BufReader::new(c);
        for (id, method) in [(3, "ping"), (4, "pong")] {
            let mut line = String::new();
            r.read_line(&mut line).unwrap();
            let v: Value = serde_json::from_str(&line).unwrap();
            assert_eq!((v["id"].as_i64(), v["method"].as_str()), (Some(id), Some(method)), "{line}");
        }
    }

    #[test]
    fn extra_connections_are_refused() {
        let addr = serve_echo();
        // Idle connections are accepted in order and held open.
        let held: Vec<TcpStream> = (0..Limits::default().max_clients).map(|_| TcpStream::connect(addr).unwrap()).collect();
        let mut extra = TcpStream::connect(addr).unwrap();
        extra.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut line = String::new();
        BufReader::new(&mut extra).read_line(&mut line).unwrap();
        assert!(line.contains("too many control connections"), "{line}");
        drop(held);
    }
}
